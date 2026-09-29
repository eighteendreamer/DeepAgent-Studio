//! Composer attachment persistence and text extraction.
//!
//! The desktop webview can provide pasted text, image/file data URLs, or an
//! OS path when available. This service normalizes all of those into a durable
//! attachment folder and reuses `FilePreviewService` for model-readable text.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::MessageAttachment;
use deepagent_runtime::AttachmentDataResolver;
use sha2::{Digest, Sha256};

use crate::dto::{AttachmentDto, AttachmentIngestDto, PreviewResultDto};
use crate::file_preview_service::FilePreviewService;

const PENDING_SESSION: &str = "pending";

#[derive(Debug, Clone)]
pub struct AttachmentService {
    root: PathBuf,
    preview: FilePreviewService,
}

struct AttachmentExtraction {
    extracted_text: Option<String>,
    preview: Option<PreviewResultDto>,
    status: String,
    message: Option<String>,
}

impl AttachmentService {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            preview: FilePreviewService::new(),
        }
    }

    pub fn ingest(&self, input: AttachmentIngestDto) -> Result<AttachmentDto> {
        let id = input
            .id
            .as_deref()
            .map(sanitize_segment)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(new_attachment_id);
        let session_bucket = input
            .session_id
            .as_deref()
            .map(sanitize_segment)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| PENDING_SESSION.to_string());
        let storage_dir = self.root.join(session_bucket).join(&id);
        fs::create_dir_all(&storage_dir)
            .map_err(|e| CoreError::Other(format!("create attachment dir: {e}")))?;

        let name = clean_file_name(&input.name);
        let original_path = self.persist_original(&storage_dir, &name, &input)?;
        let original_path_string = original_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let (size_bytes, sha256) = if let Some(path) = original_path.as_ref() {
            let bytes =
                fs::read(path).map_err(|e| CoreError::Other(format!("read attachment: {e}")))?;
            (bytes.len() as u64, Some(hex_sha256(&bytes)))
        } else {
            (
                input.text.as_ref().map(|s| s.len()).unwrap_or_default() as u64,
                None,
            )
        };

        let extraction = self.extract(&input.kind, input.text.as_deref(), original_path.as_deref());
        let extraction = match extraction {
            Ok(extracted) => extracted,
            Err(err) => AttachmentExtraction {
                extracted_text: None,
                preview: None,
                status: "error".to_string(),
                message: Some(format!("attachment extraction failed: {err}")),
            },
        };

        if let Some(text) = extraction.extracted_text.as_ref() {
            fs::write(storage_dir.join("extracted.txt"), text)
                .map_err(|e| CoreError::Other(format!("write extracted text: {e}")))?;
        }

        let dto = AttachmentDto {
            id,
            session_id: input.session_id,
            kind: input.kind,
            name,
            mime: input.mime,
            size_bytes,
            source: input.source,
            storage_dir: storage_dir.to_string_lossy().into_owned(),
            original_path: original_path_string,
            extracted_text: extraction.extracted_text,
            preview: extraction.preview,
            sha256,
            status: extraction.status,
            message: extraction.message,
        };
        self.write_metadata(&storage_dir, &dto)?;
        Ok(dto)
    }

    pub fn remove(&self, session_id: Option<&str>, id: &str) -> Result<bool> {
        let session_bucket = session_id
            .map(sanitize_segment)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| PENDING_SESSION.to_string());
        let id = sanitize_segment(id);
        let path = self.root.join(session_bucket).join(id);
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&path)
            .map_err(|e| CoreError::Other(format!("remove attachment: {e}")))?;
        Ok(true)
    }

    fn persist_original(
        &self,
        storage_dir: &Path,
        name: &str,
        input: &AttachmentIngestDto,
    ) -> Result<Option<PathBuf>> {
        if let Some(text) = input.text.as_ref() {
            let path = storage_dir.join(name);
            fs::write(&path, text)
                .map_err(|e| CoreError::Other(format!("write text attachment: {e}")))?;
            return Ok(Some(path));
        }

        if let Some(data_url) = input.data_url.as_ref() {
            let bytes = decode_data_url(data_url)?;
            let path = storage_dir.join(name);
            fs::write(&path, bytes)
                .map_err(|e| CoreError::Other(format!("write attachment data: {e}")))?;
            return Ok(Some(path));
        }

        if let Some(source) = input.local_path.as_ref() {
            let source_path = Path::new(source);
            let path = storage_dir.join(name);
            fs::copy(source_path, &path)
                .map_err(|e| CoreError::Other(format!("copy attachment '{source}': {e}")))?;
            return Ok(Some(path));
        }

        Ok(None)
    }

    fn extract(
        &self,
        kind: &str,
        text: Option<&str>,
        original_path: Option<&Path>,
    ) -> Result<AttachmentExtraction> {
        if let Some(text) = text {
            return Ok(AttachmentExtraction {
                extracted_text: Some(text.to_string()),
                preview: None,
                status: "ready".to_string(),
                message: None,
            });
        }
        if kind == "image" {
            return Ok(AttachmentExtraction {
                extracted_text: None,
                preview: None,
                status: "ready".to_string(),
                message: Some("image saved; system vision extraction is pending".to_string()),
            });
        }
        let Some(path) = original_path else {
            return Ok(AttachmentExtraction {
                extracted_text: None,
                preview: None,
                status: "ready".to_string(),
                message: Some("attachment saved without readable content".to_string()),
            });
        };

        let preview = self.preview.extract_text(&path.to_string_lossy())?;
        let extracted = preview
            .text
            .clone()
            .or_else(|| preview.sheets.as_ref().map(|sheets| sheets_to_text(sheets)));
        let message = if extracted.is_some() {
            preview.message.clone()
        } else {
            preview
                .message
                .clone()
                .or_else(|| Some("no readable text extracted".to_string()))
        };
        Ok(AttachmentExtraction {
            extracted_text: extracted,
            preview: Some(preview),
            status: "ready".to_string(),
            message,
        })
    }

    fn write_metadata(&self, storage_dir: &Path, dto: &AttachmentDto) -> Result<()> {
        let json = serde_json::to_string_pretty(dto)
            .map_err(|e| CoreError::Other(format!("serialize attachment metadata: {e}")))?;
        fs::write(storage_dir.join("metadata.json"), json)
            .map_err(|e| CoreError::Other(format!("write attachment metadata: {e}")))?;
        Ok(())
    }
}

fn sheets_to_text(sheets: &[crate::dto::SheetPreviewDto]) -> String {
    let mut out = String::new();
    for sheet in sheets {
        out.push_str("# ");
        out.push_str(&sheet.name);
        out.push('\n');
        for row in &sheet.rows {
            out.push_str(&row.join("\t"));
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn decode_data_url(data_url: &str) -> Result<Vec<u8>> {
    let encoded = data_url
        .split_once(',')
        .map(|(_, body)| body)
        .ok_or_else(|| CoreError::Other("invalid data URL".to_string()))?;
    base64_decode(encoded.trim())
}

fn base64_decode(input: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut len = 0usize;
    for byte in input.bytes().filter(|b| !b.is_ascii_whitespace()) {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(CoreError::Other("invalid base64 data".to_string())),
        };
        chunk[len] = value;
        len += 1;
        if len == 4 {
            push_base64_chunk(&mut out, chunk)?;
            len = 0;
        }
    }
    if len != 0 {
        return Err(CoreError::Other("invalid base64 padding".to_string()));
    }
    Ok(out)
}

fn push_base64_chunk(out: &mut Vec<u8>, chunk: [u8; 4]) -> Result<()> {
    if chunk[0] == 64 || chunk[1] == 64 {
        return Err(CoreError::Other("invalid base64 padding".to_string()));
    }
    out.push((chunk[0] << 2) | (chunk[1] >> 4));
    if chunk[2] != 64 {
        out.push(((chunk[1] & 0b1111) << 4) | (chunk[2] >> 2));
    }
    if chunk[3] != 64 {
        out.push(((chunk[2] & 0b11) << 6) | chunk[3]);
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_attachment_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("att_{millis}")
}

fn sanitize_segment(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn clean_file_name(value: &str) -> String {
    let file_name = Path::new(value)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_string());
    let clean = sanitize_segment(&file_name);
    if clean.is_empty() {
        "attachment".to_string()
    } else {
        clean
    }
}

/// Resolves a persisted image attachment to a base64 data URL for provider wire
/// encoding, reading the file referenced by [`MessageAttachment::path`].
///
/// DeepSeek accepts `data:` URLs for both Chat Completions (`image_url.url`) and
/// Responses (`input_image.image_url`). A single image must be ≤ 32 MiB; larger
/// images are skipped (returns `None`) so the message falls back to its text.
pub struct FileAttachmentResolver;

impl AttachmentDataResolver for FileAttachmentResolver {
    fn data_url(&self, attachment: &MessageAttachment) -> Option<String> {
        if attachment.kind != "image" {
            return None;
        }
        let path = attachment.path.as_deref()?;
        let bytes = std::fs::read(path).ok()?;
        const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let mime = attachment
            .media_type
            .clone()
            .or_else(|| sniff_image_mime(path))
            .unwrap_or_else(|| "image/png".to_string());
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:{mime};base64,{encoded}"))
    }
}

fn sniff_image_mime(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    Some(mime.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_attachment_resolver_encodes_png_data_url() {
        use deepagent_runtime::AttachmentDataResolver;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        std::fs::write(&path, [0x89u8, b'P', b'N', b'G']).unwrap();
        let attachment = MessageAttachment {
            id: "att_1".into(),
            kind: "image".into(),
            media_type: Some("image/png".into()),
            path: Some(path.to_string_lossy().into_owned()),
        };
        let url = FileAttachmentResolver.data_url(&attachment).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn file_attachment_resolver_skips_non_image_and_missing() {
        use deepagent_runtime::AttachmentDataResolver;
        let non_image = MessageAttachment {
            id: "t".into(),
            kind: "text".into(),
            media_type: None,
            path: Some("x".into()),
        };
        assert!(FileAttachmentResolver.data_url(&non_image).is_none());
        let no_path = MessageAttachment {
            id: "i".into(),
            kind: "image".into(),
            media_type: None,
            path: None,
        };
        assert!(FileAttachmentResolver.data_url(&no_path).is_none());
    }

    #[test]
    fn ingests_plain_text_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let svc = AttachmentService::new(dir.path().to_path_buf());
        let dto = svc
            .ingest(AttachmentIngestDto {
                id: Some("a1".to_string()),
                session_id: Some("s1".to_string()),
                kind: "text".to_string(),
                name: "pasted.txt".to_string(),
                mime: "text/plain".to_string(),
                source: "paste".to_string(),
                local_path: None,
                data_url: None,
                text: Some("hello".to_string()),
            })
            .unwrap();
        assert_eq!(dto.extracted_text.as_deref(), Some("hello"));
        assert!(Path::new(&dto.storage_dir).join("metadata.json").exists());
        assert!(Path::new(&dto.storage_dir).join("extracted.txt").exists());
    }

    #[test]
    fn ingests_data_url_file_and_extracts_text() {
        let dir = tempfile::tempdir().unwrap();
        let svc = AttachmentService::new(dir.path().to_path_buf());
        let dto = svc
            .ingest(AttachmentIngestDto {
                id: Some("a2".to_string()),
                session_id: None,
                kind: "file".to_string(),
                name: "notes.md".to_string(),
                mime: "text/markdown".to_string(),
                source: "drop".to_string(),
                local_path: None,
                data_url: Some("data:text/markdown;base64,IyBoZWxsbw==".to_string()),
                text: None,
            })
            .unwrap();
        assert_eq!(dto.extracted_text.as_deref(), Some("# hello"));
        assert_eq!(dto.status, "ready");
        assert!(dto.sha256.is_some());
    }
}
