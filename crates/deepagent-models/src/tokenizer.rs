//! Pinned, offline DeepSeek prompt measurement. Never loads a Hub model or
//! resolves image bytes. These are local template estimates, not billed usage.

use std::io::Read;
use std::sync::{Arc, OnceLock};

use deepagent_core::error::{CoreError, Result};
use deepseek_recipe::request::{
    ConversionOptions, ModelVersion, ProtocolRequest, WebSearchBehavior,
};
use deepseek_recipe_encoding::v4::{dsv4::DeepseekV4Encoding, dsv41::DeepseekV41Encoding};
use deepseek_recipe_encoding::{PromptEncoding, PromptTokenizer, TokenizerEncoder};
use sha2::{Digest, Sha256};

use crate::WireMode;

/// Version families explicitly supported by the bundled official templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepSeekTokenizer {
    V4,
    V41,
}

impl DeepSeekTokenizer {
    /// Exact aliases only. The official API model table (2026-11-14) maps the
    /// old Flash IDs to V4.1-Flash-1017. Unknown/future IDs must not inherit it.
    pub fn for_model(model: &str) -> Option<Self> {
        match model {
            "deepseek-v4-pro" | "DeepSeek-V4-Pro" => Some(Self::V4),
            "deepseek-flash"
            | "deepseek-v4-flash"
            | "deepseek-v4-flash-vision-exp"
            | "DeepSeek-V4.1-Flash" => Some(Self::V41),
            _ => None,
        }
    }

    /// Includes template version and data digest so a provider anchor cannot
    /// silently survive a local encoder change. Never contains prompt text.
    pub const fn source(self) -> &'static str {
        match self {
            Self::V4 => "offline_prompt_estimate:recipe-0.2.0:v4:97d2f31b020d18b5aee5c9b3d5b4efb10ea210f3fe3f7dffe3f1cd90542d6b19",
            Self::V41 => "offline_prompt_estimate:recipe-0.2.0:v41:81f64d1248a68ce3663e07ab3ee48b851e5df0e32d27cb98e4c9a268151e8d99",
        }
    }

    fn tokenizer(self) -> Result<&'static Arc<PromptTokenizer>> {
        static V4: OnceLock<std::result::Result<Arc<PromptTokenizer>, String>> = OnceLock::new();
        static V41: OnceLock<std::result::Result<Arc<PromptTokenizer>, String>> = OnceLock::new();
        let (cell, bytes, size, digest): (_, &[u8], _, _) = match self {
            Self::V4 => (
                &V4,
                include_bytes!("../assets/deepseek-tokenizers/v4.json.gz"),
                6_367_146,
                "97d2f31b020d18b5aee5c9b3d5b4efb10ea210f3fe3f7dffe3f1cd90542d6b19",
            ),
            Self::V41 => (
                &V41,
                include_bytes!("../assets/deepseek-tokenizers/v41.json.gz"),
                6_367_247,
                "81f64d1248a68ce3663e07ab3ee48b851e5df0e32d27cb98e4c9a268151e8d99",
            ),
        };
        cell.get_or_init(|| load_tokenizer(bytes, size, digest))
            .as_ref()
            .map_err(|error| CoreError::other(format!("offline_tokenizer_load: {error}")))
    }

    /// Plain text, without treating user-written control-token spellings as
    /// template tokens. No padding, truncation, BOS or EOS is added.
    pub fn count_text(self, text: &str) -> Result<usize> {
        self.tokenizer()?
            .encode_ids(text, &[])
            .map(|ids| ids.len())
            .map_err(|error| CoreError::other(format!("offline_tokenizer_encode: {error}")))
    }

    /// Count the existing serializer's JSON with the matching official prompt
    /// template. Image placeholders are removed from this text total; the host
    /// adds its routed image reserve separately. No image is decoded/fetched.
    pub(crate) fn count_wire(self, wire: serde_json::Value, mode: WireMode) -> Result<usize> {
        let model = wire["model"].as_str().unwrap_or_default();
        let version = match self {
            Self::V4 => ModelVersion::DeepseekV4Pro,
            Self::V41 => ModelVersion::DeepseekV41Flash,
        };
        let options = ConversionOptions::default()
            .with_model_alias(model, version)
            .with_responses_web_search(WebSearchBehavior::Reject);
        let converted = match mode {
            WireMode::Responses => {
                serde_json::from_value::<deepseek_recipe::openai::ResponsesRequest>(wire)
                    .map_err(CoreError::from)?
                    .convert(options)
            }
            WireMode::ChatCompletions => {
                serde_json::from_value::<deepseek_recipe::openai::ChatCompletionRequest>(wire)
                    .map_err(CoreError::from)?
                    .convert(options)
            }
            WireMode::Messages => {
                return Err(CoreError::other(
                    "offline_prompt_conversion: Messages tokenizer template is not available",
                ))
            }
        }
        .map_err(|error| CoreError::other(format!("offline_prompt_conversion: {error}")))?;
        let tokenizer = self.tokenizer()?;
        let encoding: Box<dyn PromptEncoding> = match self {
            Self::V4 => Box::new(DeepseekV4Encoding::new().with_tokenizer(tokenizer.clone())),
            Self::V41 => Box::new(DeepseekV41Encoding::new().with_tokenizer(tokenizer.clone())),
        };
        let rendered = encoding.render_conversation(&converted.conversation);
        tokenizer
            .encode_ids(&rendered.prompt, &rendered.special_ranges)
            .map(|ids| ids.len().saturating_sub(rendered.image_sources.len()))
            .map_err(|error| CoreError::other(format!("offline_prompt_encode: {error}")))
    }
}

fn load_tokenizer(
    bytes: &[u8],
    size: usize,
    digest: &str,
) -> std::result::Result<Arc<PromptTokenizer>, String> {
    let mut raw = Vec::with_capacity(size);
    flate2::read::GzDecoder::new(bytes)
        .take(size as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| format!("gzip: {error}"))?;
    if raw.len() != size || format!("{:x}", Sha256::digest(&raw)) != digest {
        return Err("bundled tokenizer length or SHA-256 mismatch".into());
    }
    let tokenizer = tokenizers::Tokenizer::from_bytes(&raw).map_err(|error| error.to_string())?;
    if tokenizer.get_padding().is_some() || tokenizer.get_truncation().is_some() {
        return Err("bundled tokenizer must not pad or truncate".into());
    }
    Ok(Arc::new(PromptTokenizer::new(&tokenizer)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checked_assets_reproduce_the_original_offline_samples() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../deepagent-context/tests/fixtures/deepseek_tokenizer_samples.json"
        ))
        .unwrap();
        for tokenizer in [DeepSeekTokenizer::V4, DeepSeekTokenizer::V41] {
            for sample in fixture["samples"].as_array().unwrap() {
                let text = sample["text"]
                    .as_str()
                    .unwrap()
                    .repeat(sample["repeat"].as_u64().unwrap() as usize);
                assert_eq!(
                    tokenizer.count_text(&text).unwrap() as u64,
                    sample["offline_tokens"].as_u64().unwrap()
                );
            }
        }
    }

    #[test]
    fn digest_damage_and_gzip_damage_are_errors_not_zero_tokens() {
        let bytes = include_bytes!("../assets/deepseek-tokenizers/v4.json.gz");
        assert!(load_tokenizer(bytes, 6_367_146, "wrong")
            .err()
            .unwrap()
            .contains("SHA-256"));
        assert!(load_tokenizer(b"broken", 6_367_146, "wrong").is_err());
    }

    #[test]
    fn exact_aliases_only_and_plain_special_spellings_are_not_control_tokens() {
        assert_eq!(
            DeepSeekTokenizer::for_model("deepseek-v4-flash"),
            Some(DeepSeekTokenizer::V41)
        );
        assert_eq!(DeepSeekTokenizer::for_model("deepseek-v99-pro"), None);
        assert_eq!(
            DeepSeekTokenizer::for_model("my-deepseek-flash-proxy"),
            None
        );
        for tokenizer in [DeepSeekTokenizer::V4, DeepSeekTokenizer::V41] {
            assert_eq!(tokenizer.count_text("").unwrap(), 0);
            assert!(tokenizer.count_text("<｜end▁of▁sentence｜>").unwrap() > 1);
            let count = tokenizer.count_wire(json!({"model":"DeepSeek-V4-Pro", "input":[{"role":"user","content":"hello"}],"reasoning":{"effort":"none"}}), WireMode::Responses).unwrap();
            assert!(count > tokenizer.count_text("hello").unwrap());
        }
    }
}
