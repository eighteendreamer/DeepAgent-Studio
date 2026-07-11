//! Third-party system-vision service.
//!
//! Image attachments are sent to an OpenAI-compatible vision endpoint
//! (ModelScope by default). The returned description is then injected into the
//! text-only main model context by the desktop app.

use std::path::PathBuf;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

use crate::dto::{VisionRecognizeRequestDto, VisionRecognizeResultDto};
use crate::settings::{SettingsService, VisionMode};
use crate::vision_cache_service::{VisionCacheEntry, VisionCacheService};
use crate::vision_provider_service::{
    default_vision_prompt, hash_bytes, VisionProviderRequest, VisionProviderService,
    VISION_PROMPT_VERSION,
};

#[derive(Clone)]
pub struct VisionService {
    settings: Arc<SettingsService>,
    provider: VisionProviderService,
    cache: VisionCacheService,
}

impl VisionService {
    pub fn new(settings: Arc<SettingsService>, cache_root: PathBuf) -> Self {
        Self {
            settings,
            provider: VisionProviderService::new(),
            cache: VisionCacheService::new(cache_root),
        }
    }

    pub async fn recognize_image(
        &self,
        request: VisionRecognizeRequestDto,
    ) -> Result<VisionRecognizeResultDto> {
        let image_path = PathBuf::from(&request.image_path);
        if !image_path.is_file() {
            return Err(CoreError::Other(format!(
                "image file not found: {}",
                image_path.display()
            )));
        }

        let settings = self.settings.vision_settings()?;
        if settings.mode == VisionMode::Off {
            return Err(CoreError::Other("system vision is disabled".to_string()));
        }
        let api_key = self
            .settings
            .vision_api_key()?
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                CoreError::Other("system vision API key is not configured".to_string())
            })?;
        if settings.system_model.trim().is_empty() {
            return Err(CoreError::Other(
                "system vision model name is not configured".to_string(),
            ));
        }

        let image_bytes =
            std::fs::read(&image_path).map_err(|e| CoreError::Other(format!("read image: {e}")))?;
        let image_hash = hash_bytes(&image_bytes);
        let prompt = request
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_vision_prompt().to_string());
        let cache_key = self.cache.key_for(
            &image_hash,
            &settings.provider,
            &settings.base_url,
            &settings.system_model,
            VISION_PROMPT_VERSION,
            &prompt,
        );

        if let Some(cached) = self.cache.get(&cache_key)? {
            return Ok(VisionRecognizeResultDto {
                model_id: cached.model,
                model_path: cached.base_url,
                text: cached.result,
                raw_json: cached.raw_json,
            });
        }

        let mime = mime_from_path(&image_path);
        let response = self
            .provider
            .recognize_image(VisionProviderRequest {
                base_url: settings.base_url.clone(),
                api_key,
                model: settings.system_model.clone(),
                timeout_ms: settings.timeout_ms,
                prompt,
                image_mime: mime.to_string(),
                image_bytes,
            })
            .await?;

        self.cache.put(
            &cache_key,
            &VisionCacheEntry {
                image_hash,
                provider: settings.provider,
                base_url: response.base_url.clone(),
                model: response.model.clone(),
                prompt_version: VISION_PROMPT_VERSION.to_string(),
                result: response.text.clone(),
                raw_json: response.raw_json.clone(),
                created_at_ms: now_ms(),
            },
        )?;

        Ok(VisionRecognizeResultDto {
            model_id: response.model,
            model_path: response.base_url,
            text: response.text,
            raw_json: response.raw_json,
        })
    }

    pub async fn test_connection(&self) -> Result<VisionRecognizeResultDto> {
        let settings = self.settings.vision_settings()?;
        let api_key = self
            .settings
            .vision_api_key()?
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                CoreError::Other("system vision API key is not configured".to_string())
            })?;
        let response = self
            .provider
            .recognize_image_url(
                &settings.base_url,
                &api_key,
                &settings.system_model,
                settings.timeout_ms,
                default_vision_prompt(),
                "https://modelscope.oss-cn-beijing.aliyuncs.com/demo/images/audrey_hepburn.jpg",
            )
            .await?;
        Ok(VisionRecognizeResultDto {
            model_id: response.model,
            model_path: response.base_url,
            text: response.text,
            raw_json: response.raw_json,
        })
    }
}

fn mime_from_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

fn now_ms() -> i64 {
    use deepagent_core::clock::{Clock, SystemClock};
    SystemClock.now().as_millis()
}
