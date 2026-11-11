//! MCP image admission into the existing attachment store, before publication.

use base64::Engine as _;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::MessageAttachment;
use deepagent_mcp::adapter::{McpImage, McpImageStore};

use crate::attachment_service::AttachmentService;
use crate::dto::AttachmentIngestDto;

const BUCKET: &str = "mcp-tool-results";
// Local admission policy, not a provider capability claim. Bound the whole
// result before decoding or writing its first image.
const MAX_IMAGES: usize = 16;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct McpAttachmentStore(pub(crate) AttachmentService);

impl McpImageStore for McpAttachmentStore {
    fn ingest(&self, images: &[McpImage<'_>]) -> Result<Vec<MessageAttachment>> {
        let invalid = |reason: &str| CoreError::invalid(format!("MCP tool image {reason}"));
        if images.len() > MAX_IMAGES {
            return Err(invalid("count exceeds local admission limit"));
        }
        let mut total = 0usize;
        for image in images {
            if !matches!(
                image.mime_type,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ) {
                return Err(invalid("has unsupported MIME type"));
            }
            if image.data.len() > MAX_TOTAL_BYTES.div_ceil(3) * 4 {
                return Err(invalid("exceeds local byte limit"));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(image.data)
                .map_err(|_| invalid("has invalid base64"))?;
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| invalid("size overflow"))?;
            if bytes.is_empty() || total > MAX_TOTAL_BYTES {
                return Err(invalid("exceeds local byte limit or is empty"));
            }
            deepagent_models::image_input::validate_image_bytes(&bytes, Some(image.mime_type))?;
        }
        let mut admitted = Vec::new();
        let mut ids = Vec::new();
        for image in images {
            let id = format!("mcp_{}", uuid::Uuid::new_v4().simple());
            ids.push(id.clone());
            let name = match image.mime_type {
                "image/png" => "original.png",
                "image/jpeg" => "original.jpg",
                "image/gif" => "original.gif",
                _ => "original.webp",
            };
            let result = self.0.ingest(AttachmentIngestDto {
                id: Some(id),
                session_id: Some(BUCKET.into()),
                kind: "image".into(),
                name: name.into(),
                mime: image.mime_type.into(),
                source: "mcp-untrusted".into(),
                local_path: None,
                data_url: Some(format!("data:{};base64,{}", image.mime_type, image.data)),
                text: None,
            });
            match result {
                Ok(dto) => admitted.push(MessageAttachment {
                    id: dto.id,
                    kind: dto.kind,
                    media_type: Some(dto.mime),
                    path: dto.original_path,
                    sha256: dto.sha256,
                }),
                Err(_) => {
                    for id in ids {
                        if self.0.remove(Some(BUCKET), &id).is_err() {
                            tracing::warn!("unpublished MCP image cleanup failed; host storage needs inspection");
                        }
                    }
                    return Err(invalid("could not be persisted in host attachment storage"));
                }
            }
        }
        Ok(admitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachment_service::FileAttachmentResolver;
    use deepagent_runtime::AttachmentDataResolver;

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=";

    #[test]
    fn h127_image_admission_is_bounded_and_persists_verifiable_refs() {
        let fixture = tempfile::tempdir().unwrap();
        let store = McpAttachmentStore(AttachmentService::new(fixture.path().join("attachments")));
        let refs = store
            .ingest(&[
                McpImage {
                    mime_type: "image/png",
                    data: PNG,
                },
                McpImage {
                    mime_type: "image/png",
                    data: PNG,
                },
            ])
            .unwrap();
        assert_eq!(refs.len(), 2);
        assert_ne!(
            refs[0].id, refs[1].id,
            "two occurrences must not overwrite their identities"
        );
        for reference in &refs {
            assert_eq!(reference.sha256.as_ref().unwrap().len(), 64);
            assert_eq!(
                FileAttachmentResolver
                    .try_data_url(reference)
                    .unwrap()
                    .unwrap(),
                format!("data:image/png;base64,{PNG}")
            );
            let metadata = std::fs::read_to_string(
                std::path::Path::new(reference.path.as_ref().unwrap())
                    .parent()
                    .unwrap()
                    .join("metadata.json"),
            )
            .unwrap();
            assert!(metadata.contains("mcp-untrusted"));
            assert!(!metadata.contains(PNG));
        }
        let frozen = serde_json::to_string(&refs).unwrap();
        drop(store);
        let restored: Vec<MessageAttachment> = serde_json::from_str(&frozen).unwrap();
        assert_eq!(restored, refs);
        assert!(restored
            .iter()
            .all(|reference| FileAttachmentResolver.try_data_url(reference).is_ok()));
        std::fs::write(restored[0].path.as_ref().unwrap(), b"tampered").unwrap();
        assert!(FileAttachmentResolver.try_data_url(&restored[0]).is_err());
    }

    #[test]
    fn h127_bad_mime_base64_signature_count_and_size_write_nothing() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("attachments");
        let store = McpAttachmentStore(AttachmentService::new(root.clone()));
        for (mime, data) in [
            ("text/plain", PNG),
            ("image/png", "not-base64"),
            ("image/jpeg", PNG),
            ("image/png", ""),
            ("image/png", "iVBORw0KGgo="),
            ("image/png", "PHN2Zy8+"),
        ] {
            let error = store
                .ingest(&[
                    McpImage {
                        mime_type: "image/png",
                        data: PNG,
                    },
                    McpImage {
                        mime_type: mime,
                        data,
                    },
                ])
                .unwrap_err()
                .to_string();
            assert!(!error.contains(PNG));
            assert!(
                !root.exists(),
                "all images must validate before the first write"
            );
        }
        let many: Vec<_> = (0..MAX_IMAGES + 1)
            .map(|_| McpImage {
                mime_type: "image/png",
                data: PNG,
            })
            .collect();
        assert!(store.ingest(&many).is_err());
        let oversized = "A".repeat(MAX_TOTAL_BYTES.div_ceil(3) * 4 + 1);
        assert!(store
            .ingest(&[McpImage {
                mime_type: "image/png",
                data: &oversized
            }])
            .is_err());
        assert!(!root.exists());
    }
}
