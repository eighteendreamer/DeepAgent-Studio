//! Files API projection and bounded retry at the existing provider boundary.
use super::*;

pub(crate) enum ImageRequest<'a> {
    Responses(&'a mut ResponseRequest),
    Chat(&'a mut ChatCompletionRequest),
}

impl ImageRequest<'_> {
    fn images(&self) -> Vec<&deepagent_core::message::MessageAttachment> {
        match self {
            Self::Responses(request) => request
                .input
                .iter()
                .flat_map(ResponseInputItem::image_attachments)
                .collect(),
            Self::Chat(request) => request
                .messages
                .iter()
                .flat_map(|message| &message.attachments)
                .filter(|attachment| attachment.kind == "image")
                .collect(),
        }
    }

    fn model(&self) -> &str {
        match self {
            Self::Responses(request) => &request.model,
            Self::Chat(request) => &request.model,
        }
    }

    fn file_ids(&self) -> &std::collections::BTreeMap<String, String> {
        match self {
            Self::Responses(request) => &request.image_file_ids,
            Self::Chat(request) => &request.image_file_ids,
        }
    }

    fn inline_urls(&self) -> &std::collections::BTreeMap<String, String> {
        match self {
            Self::Responses(request) => &request.attachment_data_urls,
            Self::Chat(request) => &request.attachment_data_urls,
        }
    }

    fn fallback_body(&mut self) -> Result<String> {
        match self {
            Self::Responses(request) => {
                request.image_file_ids.clear();
                request.validate()?;
                request_body::encode(request)
            }
            Self::Chat(request) => {
                request.image_file_ids.clear();
                request.validate()?;
                request_body::encode(request)
            }
        }
    }
}

/// Defers harmless provider lifecycle metadata until an attempt produces
/// output. This lets a rejected file reference be retried without duplicating
/// response IDs or sequence numbers in the existing runtime event stream.
struct FileReferenceRetryObserver<'a> {
    inner: &'a mut dyn crate::stream::DeltaObserver,
    pending_metadata: Vec<crate::stream::ModelStreamEvent>,
    safe_to_retry: bool,
}

impl<'a> FileReferenceRetryObserver<'a> {
    const MAX_PENDING_METADATA: usize = 256;

    fn new(inner: &'a mut dyn crate::stream::DeltaObserver) -> Self {
        Self {
            inner,
            pending_metadata: Vec::new(),
            safe_to_retry: true,
        }
    }

    fn flush(&mut self) {
        for event in self.pending_metadata.drain(..) {
            self.inner.on_event(event);
        }
    }

    fn discard(&mut self) {
        self.pending_metadata.clear();
    }
}

impl crate::stream::DeltaObserver for FileReferenceRetryObserver<'_> {
    fn on_event(&mut self, event: crate::stream::ModelStreamEvent) {
        if matches!(
            event,
            crate::stream::ModelStreamEvent::ResponseStreamEvent { .. }
        ) && self.safe_to_retry
            && self.pending_metadata.len() < Self::MAX_PENDING_METADATA
        {
            self.pending_metadata.push(event);
            return;
        }
        self.safe_to_retry = false;
        self.flush();
        self.inner.on_event(event);
    }

    fn on_usage(&mut self, usage: crate::Usage, raw: &serde_json::Value) {
        self.safe_to_retry = false;
        self.flush();
        self.inner.on_usage(usage, raw);
    }
}

impl ModelClient {
    fn files_base_url(&self) -> Result<String> {
        let mut base = url::Url::parse(&self.config.base_url)
            .map_err(|_| CoreError::invalid("deepseek_files_endpoint_invalid"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(CoreError::invalid("deepseek_files_endpoint_invalid"));
        }
        let has_version_suffix = base.path().trim_end_matches('/').ends_with("/v1");
        if has_version_suffix {
            base.path_segments_mut()
                .map_err(|_| CoreError::invalid("deepseek_files_endpoint_invalid"))?
                .pop_if_empty()
                .pop();
        }
        Ok(base.as_str().trim_end_matches('/').to_owned())
    }

    #[cfg(test)]
    pub(super) fn files_endpoint(&self) -> Result<String> {
        Ok(format!("{}/files", self.files_base_url()?))
    }

    async fn resolve_image_file_reference(
        &self,
        cache: &crate::file_references::DeepSeekImageFileCache,
        model: &str,
        declared_mime: Option<&str>,
        data_url: &str,
        now: i64,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<crate::file_references::ResolvedImageFile> {
        let (bytes, identity) = crate::image_input::decode_for_file_upload(
            data_url.to_owned(),
            declared_mime.map(str::to_owned),
            cancel.clone(),
        )
        .await?;
        let extension = match identity.media_type {
            "image/jpeg" => "jpg",
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => {
                return Err(deepagent_core::error::CoreError::invalid(
                    "unsupported image format",
                ))
            }
        };
        let files_base_url = self.files_base_url()?;
        cache
            .resolve(
                self.transport.as_ref(),
                crate::transport::FileUploadRequest {
                    url: format!("{files_base_url}/files"),
                    api_key: self.config.api_key.clone(),
                    filename: format!("image.{extension}"),
                    mime: identity.media_type.to_owned(),
                    bytes,
                },
                model,
                now,
                cancel,
            )
            .await
    }

    /// Apply one request-local Files API projection to every current and
    /// retained occurrence sharing the exact original image identity. Existing
    /// validated inline data remains available for the one bounded fallback.
    async fn prepare_file_references(
        &self,
        model: &str,
        images: &[(String, Option<String>)],
        data_urls: &std::collections::BTreeMap<String, String>,
        file_ids: &mut std::collections::BTreeMap<String, String>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        if !self.transport.supports_file_upload() {
            return Ok(());
        }
        let Some(cache) = self.image_file_cache.as_ref() else {
            return Ok(());
        };
        let mut resolved_file_ids = std::collections::BTreeMap::new();
        for (attachment_id, media_type) in images {
            let Some(url) = data_urls.get(attachment_id) else {
                continue;
            };
            if !url.starts_with("data:") {
                continue;
            }
            let file = match self
                .resolve_image_file_reference(
                    cache,
                    model,
                    media_type.as_deref(),
                    url,
                    crate::file_references::DeepSeekImageFileCache::now_unix_seconds(),
                    cancel.clone(),
                )
                .await
            {
                Ok(file) => file,
                Err(error) => {
                    if cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Acquire))
                    {
                        return Err(error);
                    }
                    file_ids.clear();
                    tracing::warn!(
                        event = "deepseek_files_upload_fallback",
                        "Files API image upload failed; using the validated inline image projection"
                    );
                    return Ok(());
                }
            };
            resolved_file_ids.insert(attachment_id.clone(), file.reference.file_id);
        }
        file_ids.extend(resolved_file_ids);
        Ok(())
    }

    pub(crate) async fn prepare_response_files_for_attempt(
        &self,
        request: &mut ResponseRequest,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        let images: Vec<_> = request
            .input
            .iter()
            .flat_map(ResponseInputItem::image_attachments)
            .map(|attachment| (attachment.id.clone(), attachment.media_type.clone()))
            .collect();
        self.prepare_file_references(
            &request.model,
            &images,
            &request.attachment_data_urls,
            &mut request.image_file_ids,
            cancel,
        )
        .await?;
        if let Err(error) = request
            .validate()
            .and_then(|_| self.validate_response_input_modalities(request))
        {
            request.image_file_ids.clear();
            return Err(error);
        }
        if let Err(error) = request_body::encode(request) {
            request.image_file_ids.clear();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn prepare_chat_files_for_attempt(
        &self,
        request: &mut ChatCompletionRequest,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        let images: Vec<_> = request
            .messages
            .iter()
            .flat_map(|message| &message.attachments)
            .filter(|attachment| attachment.kind == "image")
            .map(|attachment| (attachment.id.clone(), attachment.media_type.clone()))
            .collect();
        self.prepare_file_references(
            &request.model,
            &images,
            &request.attachment_data_urls,
            &mut request.image_file_ids,
            cancel,
        )
        .await?;
        request.validate()
    }

    async fn invalidate_request_file_ids(
        &self,
        request: &ImageRequest<'_>,
        cancel: Option<Arc<AtomicBool>>,
    ) {
        if let Some(cache) = &self.image_file_cache {
            for attachment in request.images() {
                let Some(file_id) = request.file_ids().get(&attachment.id) else {
                    continue;
                };
                let Some(url) = request.inline_urls().get(&attachment.id) else {
                    continue;
                };
                let Ok((bytes, identity)) = crate::image_input::decode_for_file_upload(
                    url.to_owned(),
                    attachment.media_type.clone(),
                    cancel.clone(),
                )
                .await
                else {
                    continue;
                };
                let Ok(files_base_url) = self.files_base_url() else {
                    continue;
                };
                if cache
                    .invalidate(
                        &format!("{files_base_url}/files"),
                        &self.config.api_key,
                        request.model(),
                        identity.media_type,
                        &bytes,
                        file_id,
                    )
                    .await
                    .is_err()
                {
                    tracing::warn!(
                            event = "deepseek_files_reference_invalidation_failed",
                            "could not invalidate a rejected Files API reference; falling back inline for this request"
                        );
                }
            }
        }
    }

    pub(crate) async fn stream_files_aware_attempt(
        &self,
        codec: &dyn crate::protocol::WireCodec,
        mut request: ImageRequest<'_>,
        transport_request: &mut TransportRequest,
        observer: &mut dyn crate::stream::DeltaObserver,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Response> {
        let mut retry_observer = FileReferenceRetryObserver::new(observer);
        let result = self
            .stream_attempt(
                codec,
                transport_request.clone(),
                &mut retry_observer,
                cancel.clone(),
            )
            .await;
        let invalid_file_reference = result.as_ref().is_err_and(Self::invalid_file_reference);
        let can_retry = invalid_file_reference
            && retry_observer.safe_to_retry
            && !cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire));
        if !invalid_file_reference {
            retry_observer.flush();
            return result;
        }

        self.invalidate_request_file_ids(&request, cancel.clone())
            .await;
        if !can_retry {
            retry_observer.flush();
            return result;
        }

        retry_observer.discard();
        drop(retry_observer);
        if cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Err(CoreError::other("request cancelled"));
        }
        transport_request.body = request.fallback_body()?;
        self.stream_attempt(codec, transport_request.clone(), observer, cancel)
            .await
    }

    fn invalid_file_reference(error: &CoreError) -> bool {
        let CoreError::Provider {
            status,
            code,
            message,
            ..
        } = error
        else {
            return false;
        };
        if !matches!(status, None | Some(400) | Some(404)) {
            return false;
        }
        // A local retry classifier, not a claim that these are documented
        // provider codes. Generic invalid_request_error is insufficient.
        let explicit_code = code.as_deref().is_some_and(|code| {
            matches!(
                code,
                "invalid_file_id" | "file_id_expired" | "file_id_not_found" | "file_id_deleted"
            )
        });
        let message = message.to_ascii_lowercase();
        explicit_code
            || [
                "invalid file_id",
                "file_id is invalid",
                "file_id expired",
                "file_id has expired",
                "file_id not found",
                "file_id was deleted",
                "file_id unavailable",
                "file_id is unavailable",
            ]
            .iter()
            .any(|needle| message.contains(needle))
    }
}
