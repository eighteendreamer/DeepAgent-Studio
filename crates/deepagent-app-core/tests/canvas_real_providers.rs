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

use deepagent_app_core::canvas_artifact_service::CanvasArtifactService;
use deepagent_app_core::canvas_model_gateway::{
    CanvasModelGateway, CanvasModelRequest, CanvasOperation,
};
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
};
use deepagent_app_core::secret_store::MemorySecretStore;
use deepagent_persistence::Database;
use deepagent_runtime::agent::Agent;
use deepagent_runtime::events::{RuntimeEvent, RuntimeEventSink};
use deepagent_runtime::workflow::{
    compile, CanvasModelBridge, CanvasVideoRequest, NodeEventPublisher, WorkflowAgent,
    WorkflowDefinition, WorkflowEdgeSpec, WorkflowNodeSpec,
};
use serde_json::{json, Map, Value};
use std::sync::Mutex;

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

/// Temp-backed artifact store; the caller removes the directory.
fn artifact_store() -> (std::path::PathBuf, Arc<CanvasArtifactService>) {
    let root =
        std::env::temp_dir().join(format!("deepagent-canvas-artifacts-{}", std::process::id()));
    let service = CanvasArtifactService::new(
        &root,
        Arc::new(Database::open_in_memory().expect("in-memory db")),
    )
    .expect("artifact service");
    (root, Arc::new(service))
}

fn model(id: &str, scenarios: Vec<CanvasScenario>) -> CanvasModelConfig {
    CanvasModelConfig {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        enabled: true,
        scenarios,
        priority: 0,
        max_reference_images: None,
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

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn openai_compatible_chat_answers() {
    let key = env("CANVAS_TEST_OPENAI_KEY")
        .expect("CANVAS_TEST_OPENAI_KEY must be set to run this live provider test");
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

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn anthropic_messages_answer_and_keep_reasoning() {
    let key = env("CANVAS_TEST_ANTHROPIC_KEY")
        .expect("CANVAS_TEST_ANTHROPIC_KEY must be set to run this live provider test");
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
        audio: Default::default(),
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

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn gemini_model_discovery_lists_the_real_catalog() {
    let key = env("CANVAS_TEST_GEMINI_KEY")
        .expect("CANVAS_TEST_GEMINI_KEY must be set to run this live provider test");
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
    let catalog = gateway
        .discover_models(&provider_id)
        .await
        .expect("gemini gateway must serve its model catalog");
    assert!(!catalog.is_empty(), "empty catalog for {base}");
    assert!(
        catalog
            .iter()
            .all(|id| !id.trim().is_empty() && !id.starts_with("models/")),
        "ids must be usable as-is, got {catalog:?}"
    );
}

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn gemini_generate_content_answers() {
    let key = env("CANVAS_TEST_GEMINI_KEY")
        .expect("CANVAS_TEST_GEMINI_KEY must be set to run this live provider test");
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

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn embedding_model_returns_a_real_vector() {
    let key = env("CANVAS_TEST_EMBED_KEY")
        .expect("CANVAS_TEST_EMBED_KEY must be set to run this live provider test");
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
        audio: Default::default(),
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

/// Captures what the run actually published, because node outputs travel on
/// events rather than in the agent's terminal message.
#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<String>>,
}

impl RuntimeEventSink for CapturingSink {
    fn emit(&self, event: RuntimeEvent) {
        self.events
            .lock()
            .expect("sink lock")
            .push(serde_json::to_string(&event).unwrap_or_default());
    }
}

/// The creative-mode golden path end to end: a text node feeds a generated
/// prompt into an image node through one edge, executed by the real
/// `WorkflowAgent` against the real providers.
#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn creative_text_then_image_graph_runs_on_real_providers() {
    let text_key = env("CANVAS_TEST_ANTHROPIC_KEY")
        .expect("CANVAS_TEST_ANTHROPIC_KEY must be set to run this live provider test");
    let image_key = env("CANVAS_TEST_OPENAI_KEY")
        .expect("CANVAS_TEST_OPENAI_KEY must be set to run this live provider test");
    let text_model = "deepseek-flash".to_string();
    let image_model = "gpt-image-2".to_string();
    let providers = gateway();
    let text_provider = add_provider(
        &providers,
        "deepseek-anthropic",
        "anthropic",
        "https://api.deepseek.com/anthropic",
        &text_key,
        vec![model(&text_model, vec![CanvasScenario::Text])],
    );
    let image_provider = add_provider(
        &providers,
        "toloveu",
        "openai",
        "https://toloveu.asia/v1",
        &image_key,
        vec![model(&image_model, vec![CanvasScenario::ImageGeneration])],
    );
    let (artifact_root, artifacts) = artifact_store();
    let gateway = Arc::new(CanvasModelGateway::new(providers).with_artifacts(artifacts.clone()));

    let node = |id: &str, kind: &str, config: Value| WorkflowNodeSpec {
        id: id.to_string(),
        kind: kind.to_string(),
        config: config.as_object().cloned().unwrap_or_default(),
    };
    let definition = WorkflowDefinition {
        version: 1,
        nodes: vec![
            node(
                "script-1",
                "script-gen",
                json!({
                    "model": format!("{text_provider}::{text_model}"),
                    "prompt": "给出一个适合画成海报的中文画面描述，控制在30字以内，只输出描述本身。",
                }),
            ),
            node(
                "image-1",
                "image-gen",
                json!({
                    "imageModel": format!("{image_provider}::{image_model}"),
                    "prompt": "把上游描述画成一张竖版海报",
                    "upstreamTexts": ["{{#script-1.text#}}"],
                    "size": "1024x1024",
                }),
            ),
        ],
        edges: vec![WorkflowEdgeSpec {
            id: "e1".to_string(),
            source: "script-1".to_string(),
            target: "image-1".to_string(),
            source_handle: None,
            target_handle: None,
        }],
    };

    let sink = Arc::new(CapturingSink::default());
    let mut agent = WorkflowAgent::new(
        compile(definition).expect("creative graph compiles"),
        Map::new(),
        None,
        NodeEventPublisher::new(sink.clone()),
    )
    .with_canvas_bridge(gateway.clone());

    // Drive the agent exactly like a run does: think once per step until it
    // reports completion, then read the node outputs back off the events.
    let mut steps = 0usize;
    loop {
        steps += 1;
        assert!(steps < 10, "agent did not converge in {steps} steps");
        match agent.think(steps, &[]).await.expect("step") {
            deepagent_runtime::agent::AgentDecision::Complete(_) => break,
            deepagent_runtime::agent::AgentDecision::Continue => {}
            other => panic!("unexpected decision {other:?}"),
        }
    }

    // Evidence lives on the node events: both creative nodes must report a
    // completed status, the text node an answer and the image node an artifact
    // reference — never inline bytes.
    let events = sink.events.lock().expect("sink lock").join("\n");
    assert!(
        events.contains("\"node_id\":\"script-1\"") && events.contains("\"node_id\":\"image-1\""),
        "missing node events, got: {events}"
    );
    assert!(
        events.contains("\"status\":\"completed\""),
        "no completed node event, got: {events}"
    );
    assert!(
        events.contains("artifact://"),
        "image node reported no artifact reference, got: {events}"
    );
    assert!(
        !events.contains("data:image") && !events.contains("base64"),
        "an event carried inline bytes: {events}"
    );
    let stored = std::fs::read_dir(&artifact_root)
        .expect("artifact dir")
        .filter_map(|entry| entry.ok())
        .collect::<Vec<_>>();
    assert_eq!(
        stored.len(),
        1,
        "expected one artifact file, got {stored:?}"
    );
    let bytes = std::fs::read(stored[0].path()).expect("read artifact");
    assert!(
        bytes.len() > 1024,
        "artifact file too small: {} bytes",
        bytes.len()
    );
    assert!(
        artifacts
            .record(
                stored[0]
                    .file_name()
                    .to_string_lossy()
                    .split('.')
                    .next()
                    .unwrap_or_default()
            )
            .expect("record")
            .is_some(),
        "artifact file has no index row"
    );
    let _ = std::fs::remove_dir_all(&artifact_root);
}

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn image_generation_returns_downloadable_bytes() {
    let key = env("CANVAS_TEST_OPENAI_KEY")
        .expect("CANVAS_TEST_OPENAI_KEY must be set to run this live provider test");
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
        audio: Default::default(),
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

/// 真实视频作业：会排队几分钟并消耗额度，因此默认跳过。
/// 需要验证时显式设 `CANVAS_TEST_VIDEO=1`。
#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn video_job_submits_polls_and_downloads() {
    let key = env("CANVAS_TEST_EMBED_KEY")
        .expect("CANVAS_TEST_EMBED_KEY must be set to run this live provider test");
    let base =
        env("CANVAS_TEST_VIDEO_BASE").unwrap_or_else(|| "https://api.siliconflow.cn/v1".into());
    let model_id =
        env("CANVAS_TEST_VIDEO_MODEL").unwrap_or_else(|| "Wan-AI/Wan2.2-T2V-A14B".into());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "siliconflow-video",
        "openai",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::VideoGeneration])],
    );
    let (_root, artifacts) = artifact_store();
    let gateway = CanvasModelGateway::new(providers).with_artifacts(artifacts.clone());
    let answer = gateway
        .generate_video(CanvasVideoRequest {
            model_ref: format!("{provider_id}::{model_id}"),
            prompt: "一只橘猫在雨夜的霓虹街道慢步步走过，电影感".to_string(),
            image_url: None,
            size: Some("832x480".to_string()),
            resume_task_id: None,
            timeout_ms: 600_000,
            cancel: None,
            on_progress: None,
        })
        .await
        .expect("video job");
    assert!(
        answer.artifact_uri.starts_with("artifact://"),
        "video node saw {}",
        answer.artifact_uri
    );
    assert!(
        !answer.task_id.trim().is_empty(),
        "provider task id missing"
    );
    let id = answer.artifact_uri.trim_start_matches("artifact://");
    let bytes = artifacts
        .read_bytes(id)
        .expect("read")
        .expect("the expiring provider url must have been downloaded");
    assert!(
        bytes.len() > 10_000,
        "downloaded video only {} bytes",
        bytes.len()
    );
}

/// 16 kHz 单声道 16 bit 的 440 Hz 单音，够用且不含版权素材。
fn tone_wav_data_url() -> String {
    use base64::Engine as _;
    let rate = 16_000u32;
    let samples: Vec<i16> = (0..6_400)
        .map(|i| {
            let t = i as f32 / rate as f32;
            (8_000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16
        })
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    format!(
        "data:audio/wav;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[ignore = "需要真实供应商密钥与网络；用 --ignored 显式运行"]
#[tokio::test]
async fn speech_transcription_uploads_audio_and_returns_text() {
    // 语音模型与向量模型在同一供应商账号下。
    let key = env("CANVAS_TEST_EMBED_KEY")
        .expect("CANVAS_TEST_EMBED_KEY must be set to run this live provider test");
    let base = env("CANVAS_TEST_EMBED_BASE")
        .unwrap_or_else(|| "https://api.siliconflow.cn/v1".to_string());
    let model_id = env("CANVAS_TEST_SPEECH_MODEL")
        .unwrap_or_else(|| "FunAudioLLM/SenseVoiceSmall".to_string());
    let providers = gateway();
    let provider_id = add_provider(
        &providers,
        "siliconflow-speech",
        "openai",
        &base,
        &key,
        vec![model(&model_id, vec![CanvasScenario::SpeechToText])],
    );
    let gateway = CanvasModelGateway::new(providers);
    let request = CanvasModelRequest {
        operation: CanvasOperation::SpeechTranscribe,
        prompt: String::new(),
        system_prompt: None,
        images: Vec::new(),
        texts: Vec::new(),
        size: None,
        audio: deepagent_app_core::canvas_model_gateway::CanvasAudioInput {
            reference: Some(tone_wav_data_url()),
            voice: None,
            format: None,
        },
        timeout_ms: 120_000,
    };
    let answer = gateway
        .execute_on(&provider_id, &model_id, &request)
        .await
        .expect("audio/transcriptions");
    match answer {
        deepagent_app_core::canvas_model_gateway::CanvasModelOutput::Text { text, .. } => {
            // 纯单音没有词，转写为空是合法结果；这里要证明的是整条线跑通。
            eprintln!("transcribed {model_id}: {text:?}");
        }
        other => panic!("unexpected output {other:?}"),
    }
}
