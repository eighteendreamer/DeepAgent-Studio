//! Composer attachment persistence and text extraction.
//!
//! The desktop webview can provide pasted text, image/file data URLs, or an
//! OS path when available. This service normalizes all of those into a durable
//! attachment folder and reuses `FilePreviewService` for model-readable text.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use deepagent_builtins::fs_guard::{validate_storage_name, OwnedDirectory, OwnedDirectoryRoot};
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
    storage: OwnedDirectoryRoot,
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
            storage: OwnedDirectoryRoot::new(root.clone()),
            root,
            preview: FilePreviewService::new(),
        }
    }

    pub fn ingest(&self, input: AttachmentIngestDto) -> Result<AttachmentDto> {
        let id = input.id.clone().unwrap_or_else(new_attachment_id);
        let relative = attachment_path(input.session_id.as_deref(), &id)?;
        let directory = self
            .storage
            .directory(&relative, true)
            .map_err(|e| CoreError::Other(format!("create attachment dir: {e}")))?;
        // Preserve existing DTO path spelling; only operation-time I/O uses
        // the pinned physical directory (including Windows verbatim paths).
        let storage_dir = self.root.join(relative);

        let name = clean_file_name(&input.name);
        let original_path = self.persist_original(&directory, &name, &input)?;
        let original_path_string = original_path
            .as_ref()
            .map(|_| storage_dir.join(&name).to_string_lossy().into_owned());
        let (size_bytes, sha256) = if original_path.is_some() {
            let bytes = directory
                .read_file(&name)
                .map_err(|e| CoreError::Other(format!("read attachment: {e}")))?;
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
            directory
                .write_file("extracted.txt", text.as_bytes())
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
        self.write_metadata(&directory, &dto)?;
        Ok(dto)
    }

    pub fn remove(&self, session_id: Option<&str>, id: &str) -> Result<bool> {
        let relative = attachment_path(session_id, id)?;
        self.storage
            .remove_tree(&relative)
            .map_err(|e| CoreError::Other(format!("remove attachment: {e}")))
    }

    fn persist_original(
        &self,
        directory: &OwnedDirectory,
        name: &str,
        input: &AttachmentIngestDto,
    ) -> Result<Option<PathBuf>> {
        if let Some(text) = input.text.as_ref() {
            let path = directory
                .write_file(name, text.as_bytes())
                .map_err(|e| CoreError::Other(format!("write text attachment: {e}")))?;
            return Ok(Some(path));
        }

        if let Some(data_url) = input.data_url.as_ref() {
            let bytes = decode_data_url(data_url)?;
            let path = directory
                .write_file(name, &bytes)
                .map_err(|e| CoreError::Other(format!("write attachment data: {e}")))?;
            return Ok(Some(path));
        }

        if let Some(source) = input.local_path.as_ref() {
            let bytes = fs::read(source)
                .map_err(|e| CoreError::Other(format!("read attachment source '{source}': {e}")))?;
            let path = directory
                .write_file(name, &bytes)
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

    fn write_metadata(&self, directory: &OwnedDirectory, dto: &AttachmentDto) -> Result<()> {
        let json = serde_json::to_string_pretty(dto)
            .map_err(|e| CoreError::Other(format!("serialize attachment metadata: {e}")))?;
        directory
            .write_file("metadata.json", json.as_bytes())
            .map_err(|e| CoreError::Other(format!("write attachment metadata: {e}")))?;
        Ok(())
    }
}

/// Attachment identities are not filenames to be sanitized: substitution would
/// merge distinct owners. Missing session alone means the pending bucket.
fn attachment_path(session_id: Option<&str>, id: &str) -> Result<PathBuf> {
    let bucket = session_id.unwrap_or(PENDING_SESSION);
    for (field, value) in [("session_id", bucket), ("id", id)] {
        validate_storage_name(value)
            .map_err(|e| CoreError::invalid(format!("attachment {field}: {e}")))?;
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(CoreError::invalid(format!(
                "attachment {field} must be a single ASCII identity"
            )));
        }
    }
    Ok(Path::new(bucket).join(id))
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
/// The persisted digest, not mutable metadata.json, is authoritative. Hashing
/// and encoding use the same bounded read; an unverified image never reaches
/// sampling. Legacy references remain deserializable but require reattachment.
pub struct FileAttachmentResolver;

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

impl AttachmentDataResolver for FileAttachmentResolver {
    fn is_resolvable(&self, attachment: &MessageAttachment) -> bool {
        attachment.kind == "image"
            && attachment
                .path
                .as_deref()
                .and_then(|path| std::fs::metadata(path).ok())
                .is_some_and(|metadata| {
                    metadata.is_file() && metadata.len() > 0 && metadata.len() <= MAX_IMAGE_BYTES
                })
    }

    fn data_url(&self, attachment: &MessageAttachment) -> Option<String> {
        self.try_data_url(attachment).ok().flatten()
    }

    fn try_data_url(&self, attachment: &MessageAttachment) -> Result<Option<String>> {
        if attachment.kind != "image" {
            return Ok(None);
        }
        let failure = |reason: &str| {
            CoreError::invalid(format!(
                "attachment_integrity: {reason}; reattach the image"
            ))
        };
        let expected = attachment
            .sha256
            .as_deref()
            .ok_or_else(|| failure("historical or input image has no admitted SHA-256"))?;
        if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(failure("invalid admitted SHA-256"));
        }
        let path = attachment
            .path
            .as_deref()
            .ok_or_else(|| failure("missing image path"))?;
        let file = fs::File::open(path)
            .map_err(|error| failure(&format!("open image failed: {error}")))?;
        let metadata = file
            .metadata()
            .map_err(|error| failure(&format!("inspect opened image failed: {error}")))?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_IMAGE_BYTES {
            return Err(failure(
                "image is not a nonempty regular file within the byte limit",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| failure(&format!("read image failed: {error}")))?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(failure("image changed size outside the byte limit"));
        }
        if !hex_sha256(&bytes).eq_ignore_ascii_case(expected) {
            return Err(failure("image bytes do not match admitted SHA-256"));
        }
        let mime = attachment
            .media_type
            .clone()
            .or_else(|| sniff_image_mime(path))
            .unwrap_or_else(|| "image/png".to_string());
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Some(format!("data:{mime};base64,{encoded}")))
    }
}

#[cfg(test)]
#[path = "attachment_storage_tests.rs"]
mod storage_tests;

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
            sha256: Some(hex_sha256(&[0x89u8, b'P', b'N', b'G'])),
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
            sha256: None,
            id: "t".into(),
            kind: "text".into(),
            media_type: None,
            path: Some("x".into()),
        };
        assert!(FileAttachmentResolver.data_url(&non_image).is_none());
        let no_path = MessageAttachment {
            sha256: None,
            id: "i".into(),
            kind: "image".into(),
            media_type: None,
            path: None,
        };
        assert!(FileAttachmentResolver.data_url(&no_path).is_none());
    }

    #[test]
    fn checked_image_resolution_binds_one_read_and_rejects_unverifiable_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        let original = b"synthetic content";
        fs::write(&path, original).unwrap();
        let mut reference = MessageAttachment {
            id: "fixture".into(),
            kind: "image".into(),
            media_type: Some("image/png".into()),
            path: Some(path.to_string_lossy().into_owned()),
            sha256: Some(hex_sha256(original)),
        };
        let url = FileAttachmentResolver
            .try_data_url(&reference)
            .unwrap()
            .unwrap();
        // Encoding must use exactly the buffer that passed the digest check.
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(url.split_once(',').unwrap().1)
                .unwrap(),
            original
        );
        reference.sha256 = Some(hex_sha256(original).to_uppercase());
        assert!(FileAttachmentResolver
            .try_data_url(&reference)
            .unwrap()
            .is_some());
        for bytes in [b"changed".as_slice(), b"".as_slice()] {
            fs::write(&path, bytes).unwrap();
            assert!(FileAttachmentResolver.try_data_url(&reference).is_err());
        }
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_IMAGE_BYTES + 1)
            .unwrap();
        assert!(FileAttachmentResolver.try_data_url(&reference).is_err());
        fs::remove_file(&path).unwrap();
        assert!(FileAttachmentResolver.try_data_url(&reference).is_err());
        reference.path = Some(root.path().to_string_lossy().into_owned());
        assert!(FileAttachmentResolver.try_data_url(&reference).is_err());
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

    #[test]
    fn audit_attachment_removal_stays_inside_one_attachment() {
        let mut safe = Vec::new();
        for (label, bucket, id) in [
            ("normal", "session-a", "first"),
            ("empty-id", "session-a", ""),
            ("dot-id", "session-a", "."),
            ("parent-bucket", "..", "unrelated"),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("attachments");
            let first = root.join("session-a/first");
            let second = root.join("session-a/second");
            let unrelated = fixture.path().join("unrelated");
            for dir in [&first, &second, &unrelated] {
                fs::create_dir_all(dir).unwrap();
                fs::write(dir.join("canary.txt"), "test-owned data").unwrap();
            }
            // This probe may exercise an overbroad recursive removal. Resolve
            // and verify the actual target stays within this disposable fixture.
            let actual_target = root
                .join(sanitize_segment(bucket))
                .join(sanitize_segment(id))
                .canonicalize()
                .unwrap();
            let fixture_root = fixture.path().canonicalize().unwrap();
            assert!(actual_target.starts_with(&fixture_root));
            assert_ne!(actual_target, fixture_root);

            let result = AttachmentService::new(root.clone()).remove(Some(bucket), id);
            let first_present = first.join("canary.txt").exists();
            let second_present = second.join("canary.txt").exists();
            let unrelated_present = unrelated.join("canary.txt").exists();
            eprintln!("Attachment cleanup audit: case={label} result={result:?} first={first_present} second={second_present} outside_root={unrelated_present}");
            if label == "normal" {
                assert!(result.unwrap());
                assert!(!first_present && second_present && unrelated_present);
            } else {
                assert!(
                    result.is_err(),
                    "invalid identity must be diagnosed, not treated as missing"
                );
                safe.push(first_present && second_present && unrelated_present);
            }
        }
        assert!(safe.into_iter().all(|value| value), "invalid attachment identities must not remove a bucket or a directory outside the attachment root");
    }

    #[test]
    fn audit_attachment_import_stays_inside_one_attachment() {
        let mut safe = Vec::new();
        for (label, bucket, id) in [
            ("normal", "session-a", "first"),
            ("dot-id", "session-a", "."),
            ("parent-id", "session-a", ".."),
            ("parent-bucket", "..", "unrelated"),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("attachments");
            fs::create_dir_all(root.join("session-a")).unwrap();
            fs::create_dir_all(fixture.path().join("unrelated")).unwrap();
            let target = root.join(bucket).join(id);
            fs::create_dir_all(&target).unwrap();
            // Verify the resolved destination before exercising production
            // writes; every file involved belongs to this disposable fixture.
            let resolved = target.canonicalize().unwrap();
            let fixture_root = fixture.path().canonicalize().unwrap();
            assert!(resolved.starts_with(&fixture_root));
            assert_ne!(resolved, fixture_root);
            let canary = resolved.join("canary.txt");
            fs::write(&canary, "existing test-owned data").unwrap();

            let result = AttachmentService::new(root.clone()).ingest(AttachmentIngestDto {
                id: Some(id.to_string()),
                session_id: Some(bucket.to_string()),
                kind: "text".to_string(),
                name: "canary.txt".to_string(),
                mime: "text/plain".to_string(),
                source: "paste".to_string(),
                local_path: None,
                data_url: None,
                text: Some("new attachment content".to_string()),
            });
            let overwritten = fs::read_to_string(&canary).unwrap() == "new attachment content";
            let metadata = resolved.join("metadata.json").is_file();
            let outside_root = !resolved.starts_with(root.canonicalize().unwrap());
            eprintln!("Attachment import audit: case={label} accepted={} overwritten={overwritten} metadata={metadata} outside_root={outside_root}",result.is_ok());
            if label == "normal" {
                assert!(result.is_ok() && overwritten && metadata && !outside_root);
            } else {
                safe.push(result.is_err() && !overwritten && !metadata);
            }
        }
        assert!(safe.into_iter().all(|value| value), "invalid attachment identities must be rejected before writing to the bucket, attachment root or sibling directory");
    }
}
