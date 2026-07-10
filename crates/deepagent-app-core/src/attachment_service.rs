//! Composer attachment persistence and text extraction.
//!
//! The desktop webview can provide pasted text, image/file data URLs, or an
//! OS path when available. This service normalizes all of those into a durable
//! attachment folder and reuses `FilePreviewService` for model-readable text.

use std::fs;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
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

#[cfg(test)]
mod tests {
    use super::*;

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
