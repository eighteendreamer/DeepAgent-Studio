//! One byte-level raster gate for admission, verified history and provider send.
//! No conversion, re-encoding, external fetch or model request occurs here.
use base64::Engine;
use deepagent_core::error::{CoreError, Result};
use image::{ImageFormat, ImageReader, Limits};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub const SUPPORTED_IMAGE_MIMES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
const MAX_DIMENSION: u32 = 8192;

#[derive(Clone, Copy, Debug)]
pub struct ImageIdentity {
    pub media_type: &'static str,
    pub width: u32,
    pub height: u32,
}

impl ImageIdentity {
    pub fn check_declared_mime(&self, declared: Option<&str>) -> Result<()> {
        if declared
            .filter(|mime| !mime.trim().is_empty())
            .is_some_and(|mime| !mime.trim().eq_ignore_ascii_case(self.media_type))
        {
            return Err(invalid(
                "image_input_mime_mismatch: declared MIME differs from actual raster",
            ));
        }
        Ok(())
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::invalid(message)
}

/// Decode the original raster, retaining its exact encoded identity. Width and
/// height limits are strict. Decoder allocation accounting is best-effort, not
/// an OS memory/CPU sandbox; encoded input and dimensions are separately bounded.
pub fn validate_image_bytes(bytes: &[u8], declared: Option<&str>) -> Result<ImageIdentity> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(invalid(
            "image_input_byte_limit: image must be nonempty and at most 32 MiB",
        ));
    }
    let format = image::guess_format(bytes)
        .map_err(|_| invalid("image_input_unsupported_format: unsupported or unknown raster"))?;
    let media_type = match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => {
            return Err(invalid(
                "image_input_unsupported_format: only JPEG, PNG, GIF and WebP are supported",
            ))
        }
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let raster = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => {
            invalid("image_input_resource_limit: raster exceeds decoder limits")
        }
        _ => invalid("image_input_invalid: encoded raster cannot be decoded"),
    })?;
    let identity = ImageIdentity {
        media_type,
        width: raster.width(),
        height: raster.height(),
    };
    identity.check_declared_mime(declared)?;
    Ok(identity)
}

/// Bound base64 before allocating its decoded form, and verify both MIME claims.
pub fn decode_image_data_url(
    url: &str,
    declared: Option<&str>,
) -> Result<(Vec<u8>, ImageIdentity)> {
    let (header, encoded) = url
        .split_once(',')
        .ok_or_else(|| invalid("image_input_invalid: image data URL has no payload"))?;
    let mime = header
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("image_input_invalid: expected a base64 image data URL"))?;
    if encoded.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return Err(invalid(
            "image_input_byte_limit: encoded image exceeds 32 MiB",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| invalid("image_input_invalid: malformed image base64"))?;
    let identity = validate_image_bytes(&bytes, Some(mime))?;
    identity.check_declared_mime(declared)?;
    Ok((bytes, identity))
}

fn check_cancelled(stopped: &AtomicBool, cancel: Option<&AtomicBool>) -> Result<()> {
    if stopped.load(Ordering::Acquire) || cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(CoreError::other(
            "request cancelled during image validation",
        ));
    }
    Ok(())
}

fn validate_urls<'a>(
    images: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    stopped: &AtomicBool,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    check_cancelled(stopped, cancel)?;
    let images: Vec<_> = images.collect();
    let max_dimension = if images.len() >= 15 {
        4096
    } else {
        MAX_DIMENSION
    };
    let mut checked = HashMap::new();
    for (url, declared) in images {
        check_cancelled(stopped, cancel)?;
        if url.starts_with("https://") || url.starts_with("http://") {
            // Remote bytes remain the provider's responsibility. Never add a
            // host fetch to validate a remote image or infer format from its URL.
            if declared
                .filter(|mime| !mime.trim().is_empty())
                .is_some_and(|mime| {
                    !SUPPORTED_IMAGE_MIMES
                        .iter()
                        .any(|allowed| mime.trim().eq_ignore_ascii_case(allowed))
                })
            {
                return Err(invalid(
                    "image_input_unsupported_format: unsupported declared image MIME",
                ));
            }
            continue;
        }
        let identity = match checked.get(url) {
            Some(identity) => *identity,
            None => {
                let (_, identity) = decode_image_data_url(url, declared)?;
                checked.insert(url, identity);
                identity
            }
        };
        identity.check_declared_mime(declared)?;
        check_cancelled(stopped, cancel)?;
        if identity.width > max_dimension || identity.height > max_dimension {
            return Err(invalid("image_input_dimensions_exceeded: raster exceeds this request's image dimension limit"));
        }
    }
    Ok(())
}

/// The existing async runtime owns the CPU job. Dropping the request signals
/// the job to stop before another raster, including on an outer kernel timeout.
/// A currently decoding raster is cooperative/bounded, not forcibly killed.
struct StopOnDrop(Arc<AtomicBool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn in_worker<T: Send + 'static>(
    request: T,
    cancel: Option<Arc<AtomicBool>>,
    validate: fn(&T, &AtomicBool, Option<&AtomicBool>) -> Result<()>,
) -> Result<()> {
    let stopped = Arc::new(AtomicBool::new(false));
    let _lifetime = StopOnDrop(stopped.clone());
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
        invalid("image_input_runtime_unavailable: raster decoding requires the async runtime")
    })?;
    runtime
        .spawn_blocking(move || validate(&request, &stopped, cancel.as_deref()))
        .await
        .map_err(|_| invalid("image_input_decoder_failed: raster validation job failed"))?
}

pub(crate) async fn validate_response_payloads(
    request: crate::ResponseRequest,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<()> {
    if !request
        .input
        .iter()
        .any(|item| !item.image_attachments().is_empty())
    {
        return Ok(());
    }
    in_worker(request, cancel, |request, stopped, cancel| {
        validate_urls(
            request
                .input
                .iter()
                .flat_map(|item| item.image_attachments())
                .filter_map(|attachment| {
                    request
                        .attachment_data_urls
                        .get(&attachment.id)
                        .map(|url| (url.as_str(), attachment.media_type.as_deref()))
                }),
            stopped,
            cancel,
        )
    })
    .await
}

pub(crate) async fn validate_chat_payloads(
    request: crate::ChatCompletionRequest,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<()> {
    if !request.messages.iter().any(|message| {
        message
            .attachments
            .iter()
            .any(|attachment| attachment.kind == "image")
    }) {
        return Ok(());
    }
    in_worker(request, cancel, |request, stopped, cancel| {
        validate_urls(
            request
                .messages
                .iter()
                .flat_map(|message| &message.attachments)
                .filter(|attachment| attachment.kind == "image")
                .filter_map(|attachment| {
                    request
                        .attachment_data_urls
                        .get(&attachment.id)
                        .map(|url| (url.as_str(), attachment.media_type.as_deref()))
                }),
            stopped,
            cancel,
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(format: ImageFormat) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut output, format)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn h169_four_real_rasters_are_decoded_without_reencoding() {
        for (format, mime) in [
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            let bytes = raster(format);
            let identity = validate_image_bytes(&bytes, Some(mime)).unwrap();
            assert_eq!(
                (identity.media_type, identity.width, identity.height),
                (mime, 1, 1)
            );
            let url = format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            );
            assert_eq!(decode_image_data_url(&url, Some(mime)).unwrap().0, bytes);
        }
    }

    #[test]
    fn h169_raster_mime_corruption_size_and_dimension_guards_are_real() {
        let png = raster(ImageFormat::Png);
        assert!(validate_image_bytes(&png, Some("image/jpeg")).is_err());
        assert!(validate_image_bytes(&png[..8], Some("image/png")).is_err());
        assert!(validate_image_bytes(&[], None).is_err());
        assert!(validate_image_bytes(&vec![0; MAX_IMAGE_BYTES + 1], None).is_err());
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8193, 1)
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        assert!(validate_image_bytes(&output.into_inner(), None).is_err());
        for unsupported in [
            b"<svg/>".as_slice(),
            b"BMinvalid",
            b"II\x2a\0invalid",
            b"\0\0\0\x18ftypavif",
        ] {
            assert!(validate_image_bytes(unsupported, None)
                .unwrap_err()
                .to_string()
                .contains("image_input_unsupported_format"));
        }
    }

    #[test]
    fn h169_valid_signature_does_not_hide_an_invalid_png_crc() {
        // The former MCP fixtures used this corrupt CRC as a normal image.
        let old = base64::engine::general_purpose::STANDARD.decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jZh0AAAAASUVORK5CYII="
        ).unwrap();
        assert_eq!(image::guess_format(&old).unwrap(), ImageFormat::Png);
        assert!(validate_image_bytes(&old, Some("image/png"))
            .unwrap_err()
            .to_string()
            .contains("image_input_invalid"));
    }

    #[test]
    fn h169_url_dedup_keeps_each_mime_claim_and_occurrence_dimension_budget() {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4097, 1)
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(output.into_inner())
        );
        let stopped = AtomicBool::new(false);
        validate_urls(
            (0..14).map(|_| (url.as_str(), Some("image/png"))),
            &stopped,
            None,
        )
        .unwrap();
        assert!(validate_urls(
            (0..15).map(|_| (url.as_str(), Some("image/png"))),
            &stopped,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("image_input_dimensions_exceeded"));
        assert!(validate_urls(
            [
                (url.as_str(), Some("image/png")),
                (url.as_str(), Some("image/jpeg"))
            ]
            .into_iter(),
            &stopped,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("image_input_mime_mismatch"));
    }

    #[test]
    fn h169_unused_image_cache_needs_no_decoder_or_runtime() {
        let request = crate::ResponseRequest::with_instructions_and_user_input(
            "deepseek-flash",
            "rules",
            "text",
        )
        .with_attachment_data_urls(std::collections::BTreeMap::from([(
            "unused".into(),
            "data:image/png;base64,invalid".into(),
        )]));
        futures::executor::block_on(validate_response_payloads(request, None)).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn h169_dropping_validation_releases_the_runtime_and_signals_the_blocking_job() {
        use std::time::Duration;
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let saw_stop = Arc::new(AtomicBool::new(false));
        struct ReleaseOnDrop(Arc<AtomicBool>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _release_on_failure = ReleaseOnDrop(release.clone());
        let future = in_worker(
            (started.clone(), release.clone(), saw_stop.clone()),
            None,
            |state, stopped, _| {
                state.0.store(true, Ordering::Release);
                while !state.1.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                state
                    .2
                    .store(stopped.load(Ordering::Acquire), Ordering::Release);
                Ok(())
            },
        );
        let task = tokio::spawn(future);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !started.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !saw_stop.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
}
