//! OpenAI-compatible vision provider client.

#[cfg(feature = "runtimes")]
use std::io::Cursor;
#[cfg(feature = "runtimes")]
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};
#[cfg(feature = "runtimes")]
use image::{GenericImageView, ImageFormat};
#[cfg(feature = "runtimes")]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const VISION_PROMPT_VERSION: &str = "vision-v1";
const VISION_MAX_IMAGE_SIDE: u32 = 2048;

#[derive(Debug, Clone)]
pub struct VisionProviderService;

#[derive(Debug, Clone)]
pub struct VisionProviderRequest {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_ms: u64,
    pub prompt: String,
    pub image_mime: String,
    pub image_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct VisionProviderResponse {
    pub base_url: String,
    pub model: String,
    pub text: String,
    pub raw_json: String,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: Vec<ContentPart>,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Serialize)]
struct ImageUrl {
    url: String,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Deserialize)]
struct Choice {
    message: Option<AssistantMessage>,
    delta: Option<AssistantMessage>,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: Option<MessageContent>,
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Parts(Vec<ResponseContentPart>),
}

#[cfg(feature = "runtimes")]
#[derive(Debug, Deserialize)]
struct ResponseContentPart {
    #[serde(default)]
    text: Option<String>,
}

impl VisionProviderService {
    pub fn new() -> Self {
        Self
    }

    pub async fn recognize_image(
        &self,
        request: VisionProviderRequest,
    ) -> Result<VisionProviderResponse> {
        let (image_mime, image_bytes) =
            prepare_image_for_provider(&request.image_mime, &request.image_bytes)?;
        let image_url = format!("data:{};base64,{}", image_mime, encode_base64(&image_bytes));
        self.chat_completion(
            &request.base_url,
            &request.api_key,
            &request.model,
            request.timeout_ms,
            &request.prompt,
            &image_url,
        )
        .await
    }

    pub async fn recognize_image_url(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        timeout_ms: u64,
        prompt: &str,
        image_url: &str,
    ) -> Result<VisionProviderResponse> {
        self.chat_completion(base_url, api_key, model, timeout_ms, prompt, image_url)
            .await
    }

    async fn chat_completion(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        timeout_ms: u64,
        prompt: &str,
        image_url: &str,
    ) -> Result<VisionProviderResponse> {
        #[cfg(not(feature = "runtimes"))]
        {
            let _ = (base_url, api_key, model, timeout_ms, prompt, image_url);
            return Err(CoreError::Other(
                "system vision HTTP client is not enabled in this build".to_string(),
            ));
        }

        #[cfg(feature = "runtimes")]
        {
            let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let body = ChatCompletionRequest {
                model: model.to_string(),
                stream: false,
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: vec![
                        ContentPart::Text {
                            text: prompt.to_string(),
                        },
                        ContentPart::ImageUrl {
                            image_url: ImageUrl {
                                url: image_url.to_string(),
                            },
                        },
                    ],
                }],
            };
            let body = serde_json::to_string(&body)
                .map_err(|e| CoreError::Other(format!("serialize vision request: {e}")))?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_millis(timeout_ms.max(1_000)))
                .build()
                .map_err(|e| CoreError::Other(format!("create vision HTTP client: {e}")))?;
            let resp = client
                .post(endpoint)
                .bearer_auth(api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|e| CoreError::Other(format!("vision request failed: {e}")))?;
            let status = resp.status();
            let raw = resp
                .text()
                .await
                .map_err(|e| CoreError::Other(format!("read vision response: {e}")))?;
            if !status.is_success() {
                return Err(CoreError::Other(format!(
                    "vision request failed with status {status}: {raw}"
                )));
            }
            let parsed: ChatCompletionResponse = serde_json::from_str(&raw)
                .map_err(|e| CoreError::Other(format!("parse vision response: {e}; body={raw}")))?;
            let text = parsed
                .choices
                .into_iter()
                .filter_map(|choice| choice.message.or(choice.delta))
                .filter_map(|msg| msg.content)
                .map(content_to_text)
                .find(|text| !text.trim().is_empty())
                .unwrap_or_default();
            if text.trim().is_empty() {
                return Err(CoreError::Other(
                    "vision response did not contain readable text".to_string(),
                ));
            }
            Ok(VisionProviderResponse {
                base_url: base_url.trim_end_matches('/').to_string(),
                model: model.to_string(),
                text,
                raw_json: raw,
            })
        }
    }
}

#[cfg(feature = "runtimes")]
fn prepare_image_for_provider(mime: &str, bytes: &[u8]) -> Result<(String, Vec<u8>)> {
    let image = match image::load_from_memory(bytes) {
        Ok(image) => image,
        Err(err) => {
            tracing::warn!("system vision image decode failed; sending original bytes: {err}");
            return Ok((mime.to_string(), bytes.to_vec()));
        }
    };

    let (width, height) = image.dimensions();
    if width <= VISION_MAX_IMAGE_SIDE && height <= VISION_MAX_IMAGE_SIDE {
        return Ok((mime.to_string(), bytes.to_vec()));
    }

    let resized = image.resize(
        VISION_MAX_IMAGE_SIDE,
        VISION_MAX_IMAGE_SIDE,
        image::imageops::FilterType::Lanczos3,
    );
    let mut out = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| CoreError::Other(format!("resize image for vision request: {e}")))?;
    tracing::info!(
        original_width = width,
        original_height = height,
        resized_width = resized.width(),
        resized_height = resized.height(),
        "resized image for system vision provider"
    );
    Ok(("image/png".to_string(), out))
}

#[cfg(not(feature = "runtimes"))]
fn prepare_image_for_provider(mime: &str, bytes: &[u8]) -> Result<(String, Vec<u8>)> {
    Ok((mime.to_string(), bytes.to_vec()))
}

pub fn default_vision_prompt() -> &'static str {
    "你是系统视觉识别模块。请识别用户上传的图片，并输出结构化结果。\n\n要求：\n1. 描述图片整体内容。\n2. 提取图片中可见文字。\n3. 如果是软件界面截图，说明界面结构、按钮、输入框、菜单、报错信息。\n4. 如果是图表，说明坐标、趋势、异常点。\n5. 如果是物体或场景，说明主体、背景、关键细节。\n6. 如果用户可能是在反馈 bug，请指出图片中可见的问题点。\n7. 不要猜测看不清或无法确认的信息。\n8. 用中文回答。\n\n输出格式：\n- 图片概述：\n- 可见文字：\n- 界面/物体元素：\n- 可能的问题或用户意图：\n- 不确定内容："
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(feature = "runtimes")]
fn content_to_text(content: MessageContent) -> String {
    match content {
        MessageContent::Text(text) => text,
        MessageContent::Parts(parts) => parts
            .into_iter()
            .filter_map(|part| part.text)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encoder_matches_known_values() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
    }
}
