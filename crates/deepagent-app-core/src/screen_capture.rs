//! Windows host screenshot. Registered only for local, human-approved chat runs
//! that have an image-capable tool-result protocol.
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use async_trait::async_trait;
use base64::Engine as _;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::MessageAttachment;
use deepagent_core::tool_content::ToolResultPart;
use deepagent_tools::{
    Permission, PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use serde_json::{json, Value};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
    GetDeviceCaps, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HORZRES, SRCCOPY, VERTRES,
};

use crate::attachment_service::AttachmentService;
use crate::dto::AttachmentIngestDto;

pub(crate) const NAME: &str = "screen_capture";
pub(crate) const APPROVAL_REASON: &str =
    "Capture the Windows primary display and send the screenshot to the current model for analysis.";
const BUCKET: &str = "screen-captures";
const MAX_PIXELS: usize = 16 * 1024 * 1024;

pub(crate) struct ScreenCaptureTool {
    attachments: PathBuf,
}

impl ScreenCaptureTool {
    pub(crate) fn new(attachments: PathBuf) -> Self {
        Self { attachments }
    }
}

#[async_trait]
impl Tool for ScreenCaptureTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: NAME.into(),
            description: "Capture the visible Windows primary display as an image for analysis. Use only if the user asks you to inspect their screen. The screenshot is sent to the current model after approval and saved in DeepAgent local attachment storage.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"display": {"type": "string", "enum": ["primary"]}},
                "required": ["display"],
                "additionalProperties": false
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::from_iter_perms([Permission::ReadOnly, Permission::Network]),
        }
    }

    async fn invoke(&self, arguments: Value) -> Result<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::new(Default::default()))
            .await
    }

    async fn invoke_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        if arguments.get("display").and_then(Value::as_str) != Some("primary") {
            return Err(CoreError::invalid(
                "screen capture requires display=primary",
            ));
        }
        let attachments = self.attachments.clone();
        let (png, width, height) = tokio::task::spawn_blocking(move || {
            if context.is_cancelled() {
                return Err(CoreError::other("screen capture cancelled"));
            }
            let result = capture_primary_display()?;
            if context.is_cancelled() {
                return Err(CoreError::other("screen capture cancelled"));
            }
            Ok(result)
        })
        .await
        .map_err(|error| CoreError::other(format!("screen capture worker: {error}")))??;
        admit_capture(&attachments, &png, width, height)
    }
}

fn admit_capture(root: &Path, png: &[u8], width: u32, height: u32) -> Result<ToolOutput> {
    let store = AttachmentService::new(root.to_path_buf());
    let id = format!("screen_{}", uuid::Uuid::new_v4().simple());
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let attachment = match store.ingest(AttachmentIngestDto {
        id: Some(id.clone()),
        session_id: Some(BUCKET.into()),
        kind: "image".into(),
        name: "screenshot.png".into(),
        mime: "image/png".into(),
        source: NAME.into(),
        local_path: None,
        data_url: Some(format!("data:image/png;base64,{encoded}")),
        text: None,
    }) {
        Ok(attachment) => attachment,
        Err(error) => {
            if store.remove(Some(BUCKET), &id).is_err() {
                tracing::warn!("could not clean up unpublished screen capture attachment");
            }
            return Err(error);
        }
    };
    let mut output =
        ToolOutput::success(json!({"display": "primary", "width": width, "height": height}));
    output.content = vec![
        ToolResultPart::Text {
            text: format!("Windows primary display screenshot ({width} × {height})."),
        },
        ToolResultPart::Image {
            attachment: MessageAttachment {
                id: attachment.id,
                kind: attachment.kind,
                media_type: Some(attachment.mime),
                path: attachment.original_path,
                sha256: attachment.sha256,
            },
        },
    ];
    Ok(output)
}

#[derive(Default)]
struct CaptureHandles {
    display: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for CaptureHandles {
    fn drop(&mut self) {
        // SAFETY: All non-null handles were acquired on the current thread.
        // Restore the original selection before freeing the bitmap.
        unsafe {
            if !self.previous.is_null() {
                SelectObject(self.memory, self.previous);
            }
            if !self.memory.is_null() {
                DeleteDC(self.memory);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.display.is_null() {
                ReleaseDC(null_mut(), self.display);
            }
        }
    }
}

fn capture_primary_display() -> Result<(Vec<u8>, u32, u32)> {
    let failed = |step| CoreError::other(format!("primary display capture failed at {step}"));
    // SAFETY: The DC and DIB stay on the current thread. The mapped pixel
    // buffer is read only after GdiFlush and only while the bitmap is alive.
    let (rgb, width, height) = unsafe {
        let mut handles = CaptureHandles {
            display: GetDC(null_mut()),
            ..Default::default()
        };
        if handles.display.is_null() {
            return Err(failed("GetDC"));
        }
        let width = GetDeviceCaps(handles.display, HORZRES as i32);
        let height = GetDeviceCaps(handles.display, VERTRES as i32);
        let pixels = (width as usize).saturating_mul(height as usize);
        if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) || pixels > MAX_PIXELS {
            return Err(CoreError::invalid(
                "primary display dimensions exceed the screenshot limit",
            ));
        }
        handles.memory = CreateCompatibleDC(handles.display);
        if handles.memory.is_null() {
            return Err(failed("CreateCompatibleDC"));
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut c_void = null_mut();
        handles.bitmap = CreateDIBSection(
            handles.display,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            null_mut(),
            0,
        );
        if handles.bitmap.is_null() || bits.is_null() {
            return Err(failed("CreateDIBSection"));
        }
        handles.previous = SelectObject(handles.memory, handles.bitmap);
        if handles.previous.is_null() {
            return Err(failed("SelectObject"));
        }
        if BitBlt(
            handles.memory,
            0,
            0,
            width,
            height,
            handles.display,
            0,
            0,
            SRCCOPY | CAPTUREBLT,
        ) == 0
            || GdiFlush() == 0
        {
            return Err(failed("copying pixels"));
        }
        // A 32-bit BI_RGB DIB has four bytes per pixel with no row padding.
        let bgrx = std::slice::from_raw_parts(bits.cast::<u8>(), pixels * 4);
        let rgb = bgrx
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            .collect::<Vec<_>>();
        (rgb, width as u32, height as u32)
    };
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(|error| CoreError::other(format!("encode screenshot PNG: {error}")))?;
    if png.len() > deepagent_models::image_input::MAX_IMAGE_BYTES {
        return Err(CoreError::invalid(
            "screenshot exceeds the image input size limit",
        ));
    }
    Ok((png, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachment_service::FileAttachmentResolver;
    use deepagent_runtime::AttachmentDataResolver;
    use deepagent_tools::ToolRegistry;

    #[test]
    fn screenshot_cannot_run_without_approval_or_network_permission() {
        use crate::approval_bridge::ApprovalScope;
        assert_eq!(
            ApprovalScope::offered(NAME, None),
            vec![ApprovalScope::SingleCall]
        );
        let root = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry
            .register(std::sync::Arc::new(ScreenCaptureTool::new(
                root.path().into(),
            )))
            .unwrap();
        assert!(registry
            .check(NAME, &PermissionSet::developer(), false)
            .is_err());
        assert!(registry
            .check(NAME, &PermissionSet::read_only(), true)
            .is_err());
        assert!(registry
            .check(NAME, &PermissionSet::developer(), true)
            .is_ok());
    }

    #[test]
    fn screenshot_bytes_are_resolved_from_a_structured_tool_image() {
        let root = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&[0, 127, 255], 1, 1, ExtendedColorType::Rgb8)
            .unwrap();
        let output = admit_capture(root.path(), &png, 1, 1).unwrap();
        assert!(output.ok);
        let ToolResultPart::Image { attachment } = &output.content[1] else {
            panic!("missing screenshot image")
        };
        let image_url = FileAttachmentResolver
            .try_data_url(attachment)
            .unwrap()
            .unwrap();
        assert_eq!(
            image_url,
            format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(png)
            )
        );
        assert!(!serde_json::to_string(&output).unwrap().contains("base64,"));
    }
}
