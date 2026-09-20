//! Real-provider checks for the canvas model gateway.
//!
//! These tests hit live vendor endpoints and are therefore skipped unless the
//! matching api key is present in the environment. Nothing here runs in the
//! default offline test pass, and no key is ever stored in the repository:
//!
//! ```text
//! CANVAS_TEST_OPENAI_KEY=...      CANVAS_TEST_OPENAI_BASE=...     CANVAS_TEST_OPENAI_MODEL=...
//! CANVAS_TEST_ANTHROPIC_KEY=...   CANVAS_TEST_ANTHROPIC_MODEL=...
//! CANVAS_TEST_GEMINI_KEY=...      CANVAS_TEST_GEMINI_MODEL=...
//! CANVAS_TEST_EMBED_KEY=...       CANVAS_TEST_EMBED_MODEL=...
//! ```

use std::sync::Arc;

use deepagent_app_core::canvas_model_gateway::{
    CanvasModelGateway, CanvasModelRequest, CanvasOperation,
};
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
};
use deepagent_app_core::secret_store::MemorySecretStore;
use deepagent_persistence::Database;

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn gateway() -> Arc<CanvasProviderService> {
    let db = Arc::new(Database::open_in_memory().expect("in-memory db"));
    Arc::new(CanvasProviderService::new(
        db,
        Arc::new(MemorySecretStore::default()),
    ))
}

fn model(id: &str, scenarios: Vec<CanvasScenario>) -> CanvasModelConfig {
    CanvasModelConfig {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        enabled: true,
        scenarios,
        priority: 0,
    }
}

/// Register one provider and return its id.
fn add_provider(
    providers: &CanvasProviderService,
    name: &str,
    protocol: &str,
    base_url: &str,
    api_key: &str,
    models: Vec<CanvasModelConfig>,
) -> String {
    providers
        .save_provider(CanvasProviderInput {
            id: String::new(),
            name: name.to_string(),
            protocol: protocol.to_string(),
            base_url: base_url.to_string(),
            enabled: true,
            models,
            logo: None,
            api_key: Some(api_key.to_string()),
        })
        .expect("save provider")
        .id
}

#[tokio::test]
async fn openai_compatible_chat_answers() {
    let Some(key) = env("CANVAS_TEST_OPENAI_KEY") else {
        eprintln!("skip: CANVAS_TEST_OPENAI_KEY not set");
        return;
    };
    let base = env("CANVAS_TEST_OPENAI_BASE").unwrap_or_else(|| "https://toloveu.asia/v1".into());
    let model_id = env("CANVAS_TEST_OPENAI_MODEL").unwrap_or_else(|| "gpt-5.6-sol".into());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "toloveu",
        "openai",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::Text])],
    );
    let gateway = CanvasModelGateway::new(providers);
    let answer = gateway
        .execute_on(
            &provider_id,
            &model_id,
            &CanvasModelRequest::text("回答且只回答：pong"),
        )
        .await
        .expect("openai chat");
    let text = match answer {
        deepagent_app_core::canvas_model_gateway::CanvasModelOutput::Text { text, .. } => text,
        other => panic!("unexpected output {other:?}"),
    };
    assert!(text.to_lowercase().contains("pong"), "got {text:?}");

    let discovered = gateway
        .discover_models(&provider_id)
        .await
        .expect("catalog");
    assert!(
        discovered.contains(&model_id),
        "catalog {discovered:?} lacks {model_id}"
    );
}

#[tokio::test]
async fn anthropic_messages_answer_and_keep_reasoning() {
    let Some(key) = env("CANVAS_TEST_ANTHROPIC_KEY") else {
        eprintln!("skip: CANVAS_TEST_ANTHROPIC_KEY not set");
        return;
    };
    let base = env("CANVAS_TEST_ANTHROPIC_BASE")
        .unwrap_or_else(|| "https://api.deepseek.com/anthropic".into());
    let model_id = env("CANVAS_TEST_ANTHROPIC_MODEL").unwrap_or_else(|| "deepseek-flash".into());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "deepseek-anthropic",
        "anthropic",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::Text])],
    );
    let gateway = CanvasModelGateway::new(providers);
    let request = CanvasModelRequest {
        operation: CanvasOperation::TextGenerate,
        prompt: "回答且只回答：pong".to_string(),
        system_prompt: Some("你是一个只输出一个词的助手".to_string()),
        images: Vec::new(),
        texts: Vec::new(),
        size: None,
        timeout_ms: 60_000,
    };
    let answer = gateway
        .execute_on(&provider_id, &model_id, &request)
        .await
        .expect("anthropic messages");
    match answer {
        deepagent_app_core::canvas_model_gateway::CanvasModelOutput::Text {
            text,
            reasoning,
            model_id,
            ..
        } => {
            assert!(text.to_lowercase().contains("pong"), "got {text:?}");
            assert!(!model_id.is_empty());
            // 推理型供应商会返回 thinking；有就必须保留，不能压平。
            if let Some(reasoning) = reasoning {
                assert!(!reasoning.trim().is_empty());
            }
        }
        other => panic!("unexpected output {other:?}"),
    }
}

#[tokio::test]
async fn gemini_generate_content_answers() {
    let Some(key) = env("CANVAS_TEST_GEMINI_KEY") else {
        eprintln!("skip: CANVAS_TEST_GEMINI_KEY not set");
        return;
    };
    let base = env("CANVAS_TEST_GEMINI_BASE").unwrap_or_else(|| "http://127.0.0.1:8045".into());
    let model_id = env("CANVAS_TEST_GEMINI_MODEL").unwrap_or_else(|| "gemini-3.8-flash-low".into());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "gemini-local",
        "gemini",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::Text])],
    );
    let gateway = CanvasModelGateway::new(providers);
    let answer = gateway
        .execute_on(
            &provider_id,
            &model_id,
            &CanvasModelRequest::text("回答且只回答：pong"),
        )
        .await
        .expect("gemini generateContent");
    match answer {
        deepagent_app_core::canvas_model_gateway::CanvasModelOutput::Text { text, .. } => {
            assert!(text.to_lowercase().contains("pong"), "got {text:?}");
        }
        other => panic!("unexpected output {other:?}"),
    }
}

#[tokio::test]
async fn embedding_model_returns_a_real_vector() {
    let Some(key) = env("CANVAS_TEST_EMBED_KEY") else {
        eprintln!("skip: CANVAS_TEST_EMBED_KEY not set");
        return;
    };
    let base =
        env("CANVAS_TEST_EMBED_BASE").unwrap_or_else(|| "https://api.siliconflow.cn/v1".into());
    // 模型名里带斜杠，只能出现在请求体里。
    let model_id =
        env("CANVAS_TEST_EMBED_MODEL").unwrap_or_else(|| "Qwen/Qwen3-VL-Embedding-8B".into());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "siliconflow",
        "openai",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::Embedding])],
    );
    let gateway = CanvasModelGateway::new(providers);
    let request = CanvasModelRequest {
        operation: CanvasOperation::Embedding,
        prompt: "画布向量测试".to_string(),
        system_prompt: None,
        images: Vec::new(),
        texts: vec!["画布向量测试".to_string()],
        size: None,
        timeout_ms: 60_000,
    };
    let answer = gateway
        .execute_on(&provider_id, &model_id, &request)
        .await
        .expect("embeddings call");
    match answer {
        deepagent_app_core::canvas_model_gateway::CanvasModelOutput::Embedding {
            vector,
            dimensions,
            model_id: echoed,
            ..
        } => {
            assert!(dimensions > 128, "unexpected dimension {dimensions}");
            assert_eq!(vector.len(), dimensions);
            // 斜杠模型名必须原样回显，说明它没有被拆成路由段。
            assert_eq!(echoed, "Qwen/Qwen3-VL-Embedding-8B");
        }
        other => panic!("unexpected output {other:?}"),
    }
}

#[tokio::test]
async fn image_generation_returns_downloadable_bytes() {
    let Some(key) = env("CANVAS_TEST_OPENAI_KEY") else {
        eprintln!("skip: CANVAS_TEST_OPENAI_KEY not set");
        return;
    };
    let base = env("CANVAS_TEST_OPENAI_BASE").unwrap_or_else(|| "https://toloveu.asia/v1".into());
    let model_id = env("CANVAS_TEST_IMAGE_MODEL").unwrap_or_else(|| "gpt-image-2".into());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "toloveu",
        "openai",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::ImageGeneration])],
    );
    let gateway = CanvasModelGateway::new(providers);
    let request = CanvasModelRequest {
        operation: CanvasOperation::ImageGenerate,
        prompt: "一个红色小立方体放在白色背景上".to_string(),
        system_prompt: None,
        images: Vec::new(),
        texts: Vec::new(),
        size: Some("1024x1024".to_string()),
        timeout_ms: 300_000,
    };
    let answer = gateway
        .execute_on(&provider_id, &model_id, &request)
        .await
        .expect("images.generations");
    match answer {
        deepagent_app_core::canvas_model_gateway::CanvasModelOutput::Image {
            bytes, mime, ..
        } => {
            // 供应商返回空 b64_json 时必须走 url 下载，不能产出 0 字节的“图片”。
            assert!(
                bytes.len() > 1024,
                "image payload too small: {} bytes",
                bytes.len()
            );
            assert!(mime.starts_with("image/"), "unexpected mime {mime}");
        }
        other => panic!("unexpected output {other:?}"),
    }
}
