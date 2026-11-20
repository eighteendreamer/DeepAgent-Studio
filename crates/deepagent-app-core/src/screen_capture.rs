//! Desktop application window screenshot. Registered only for local, human-approved chat runs
//! that have an image-capable tool-result protocol.
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine as _;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::MessageAttachment;
use deepagent_core::tool_content::ToolResultPart;
use deepagent_tools::{
    Permission, PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, RgbaImage};
use serde_json::{json, Value};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use crate::attachment_service::AttachmentService;
use crate::dto::AttachmentIngestDto;

pub(crate) const NAME: &str = "screen_capture";
pub(crate) const APPROVAL_REASON: &str =
    "Capture the specified application window and send the screenshot to the current model for analysis.";
const BUCKET: &str = "screen-captures";

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
            description: "Capture one application window as an image for analysis. Use only at the user’s request. Pass its exact title (ASCII case-insensitive); ask for the title if unknown. On Wayland, the user picks the window in the system dialog. No desktop or other windows are included. The screenshot is sent to the current model after approval and stored as a local attachment.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"window_title": {
                    "type": "string", "minLength": 1, "maxLength": 512,
                    "description": "Exact title of the window to capture, as shown in its title bar."
                }},
                "required": ["window_title"],
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
        let title = arguments
            .get("window_title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty() && title.encode_utf16().count() <= 512)
            .ok_or_else(|| CoreError::invalid("screen_capture requires a non-empty window_title"))?
            .to_owned();
        #[cfg(target_os = "linux")]
        let (image, captured_title) = if linux::is_wayland() {
            (
                linux::capture_portal(&context).await?,
                "window selected by the user".to_owned(),
            )
        } else {
            (capture_named_window(&title, &context).await?, title)
        };
        #[cfg(not(target_os = "linux"))]
        let (image, captured_title) = (capture_named_window(&title, &context).await?, title);

        let attachments = self.attachments.clone();
        tokio::task::spawn_blocking(move || {
            check_cancelled(&context)?;
            let (width, height) = image.dimensions();
            check_dimensions(width, height)?;
            let mut png = Vec::new();
            PngEncoder::new(&mut png)
                .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
                .map_err(|error| capture_error("encode window screenshot", error))?;
            check_cancelled(&context)?;
            admit_capture(&attachments, &captured_title, &png, width, height)
        })
        .await
        .map_err(|error| capture_error("store window screenshot", error))?
    }
}

async fn capture_named_window(title: &str, context: &ToolExecutionContext) -> Result<RgbaImage> {
    let title = title.to_owned();
    let context = context.clone();
    tokio::task::spawn_blocking(move || {
        check_cancelled(&context)?;
        #[cfg(windows)]
        return windows::capture(&title);
        #[cfg(target_os = "macos")]
        return macos::capture(&title);
        #[cfg(target_os = "linux")]
        return linux::capture_x11(&title);
    })
    .await
    .map_err(|error| capture_error("window capture worker", error))?
}

fn unique_window<T>(mut matches: impl Iterator<Item = T>) -> Result<T> {
    let window = matches
        .next()
        .ok_or_else(|| CoreError::not_found("no open window matches the exact title"))?;
    if matches.next().is_some() {
        return Err(CoreError::invalid(
            "more than one window has that title; rename or close the duplicate and retry",
        ));
    }
    Ok(window)
}

fn check_cancelled(context: &ToolExecutionContext) -> Result<()> {
    if context.is_cancelled() {
        return Err(CoreError::other("window capture cancelled"));
    }
    Ok(())
}

fn check_dimensions(width: u32, height: u32) -> Result<()> {
    if !(1..=8192).contains(&width)
        || !(1..=8192).contains(&height)
        || u64::from(width) * u64::from(height) > 16 * 1024 * 1024
    {
        return Err(CoreError::invalid(
            "window dimensions exceed the screenshot limit",
        ));
    }
    Ok(())
}

fn capture_error(step: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::other(format!("{step}: {error}"))
}

fn admit_capture(
    root: &Path,
    title: &str,
    png: &[u8],
    width: u32,
    height: u32,
) -> Result<ToolOutput> {
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
        ToolOutput::success(json!({"window_title": title, "width": width, "height": height}));
    output.content = vec![
        ToolResultPart::Text {
            text: format!("Screenshot of window {title:?} ({width} × {height})."),
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
    fn window_selection_refuses_missing_and_ambiguous_targets() {
        assert!(unique_window(std::iter::empty::<()>()).is_err());
        assert_eq!(unique_window([17].into_iter()).unwrap(), 17);
        assert!(unique_window([17, 42].into_iter()).is_err());
    }

    /// Run with a script-owned test window. It deliberately never scans or
    /// captures a user's desktop when the normal test suite is run.
    #[tokio::test]
    #[ignore = "requires a visible test window on the local desktop"]
    async fn captures_native_fixture_window() {
        let title = std::env::var("DEEPAGENT_CAPTURE_TEST_WINDOW").unwrap();
        let root = tempfile::tempdir().unwrap();
        let output = ScreenCaptureTool::new(root.path().into())
            .invoke(json!({"window_title": &title}))
            .await
            .unwrap();
        assert_eq!(output.value["window_title"], title);
        let [_, ToolResultPart::Image { attachment }] = output.content.as_slice() else {
            panic!("missing screenshot image");
        };
        let image = image::open(attachment.path.as_deref().unwrap())
            .unwrap()
            .into_rgb8();
        assert!(image.width() >= 180 && image.height() >= 100);
        assert!(
            image
                .pixels()
                .filter(|pixel| pixel.0 == [31, 139, 197])
                .count()
                > 2_000,
            "capture must contain the fixture's distinctive solid background"
        );
    }

    #[test]
    fn screenshot_bytes_are_resolved_from_a_structured_tool_image() {
        let root = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&[0, 127, 255], 1, 1, ExtendedColorType::Rgb8)
            .unwrap();
        let output = admit_capture(root.path(), "Test window", &png, 1, 1).unwrap();
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
        use deepagent_core::clock::FixedClock;
        use deepagent_core::{event::EventPayload, message::ToolCall};
        use deepagent_persistence::Database;
        use deepagent_session::Session;

        assert!(!serde_json::to_string(&output).unwrap().contains("base64,"));
        let path = root.path().join("history.sqlite");
        let id = {
            let db = Database::open(&path).unwrap();
            let clock = FixedClock::new(1_000);
            let mut session = Session::create(&db, &clock, None).unwrap();
            session
                .append(EventPayload::ToolCallRequested {
                    call: ToolCall {
                        id: "capture-1".into(),
                        name: NAME.into(),
                        arguments: json!({"window_title": "Test window"}),
                    },
                })
                .unwrap();
            session
                .append(EventPayload::ToolCallCompleted {
                    call_id: "capture-1".into(),
                    ok: true,
                    output: output.value.clone(),
                    content: output.content.clone(),
                    duration_ms: 50,
                })
                .unwrap();
            session.id().to_string()
        };
        let service = crate::service::AppService::new(Database::open(&path).unwrap());
        let conversation = service.session_conversation(&id).unwrap();
        let images = conversation
            .iter()
            .flat_map(|message| &message.parts)
            .find_map(|part| match part {
                crate::dto::ConversationPartDto::Tool { images, .. } => Some(images),
                _ => None,
            })
            .expect("screenshot tool card");
        assert_eq!(images.as_slice(), std::slice::from_ref(attachment));
    }
}
