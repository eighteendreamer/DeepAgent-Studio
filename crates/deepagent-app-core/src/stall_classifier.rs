//! DeepSeek-backed stall/laziness classifier (§2.3, main source Grok
//! `laziness_classifier.rs`; runtime-side machinery in
//! [`deepagent_runtime::stall_detector`]).
//!
//! Mirrors the [`crate::command_guard_llm`] posture exactly:
//!
//! - **Advisory.** A stalled verdict only ever injects one reference-worded
//!   `<system-reminder>` nudge (runtime cap = 1/run); it never blocks or
//!   fails a run.
//! - **Fail-open.** Any classifier error/timeout/parse failure returns `None`
//!   and the final answer completes normally.
//! - **Opt-in.** Wired only when `DEEPAGENT_STALL_DETECTOR` is truthy, so the
//!   default build adds no per-final-answer model call. A settings/UI toggle
//!   is the registered follow-up.
//! - **Lightweight.** `deepseek-chat`, temperature 0, small output budget,
//!   bounded timeout (Grok runs its classifier on a cheap sampler the same
//!   way).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use deepagent_core::message::Message;
use deepagent_models::chat::ChatRequest;
use deepagent_models::ModelClient;
use deepagent_runtime::stall_detector::{
    parse_stall_verdict, StallClassifier, StallVerdict, STALL_CLASSIFIER_PROMPT,
};

/// The environment flag that enables the stall detector.
const ENABLE_ENV: &str = "DEEPAGENT_STALL_DETECTOR";
/// Bound on the classifier round-trip; on timeout the check fails open.
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether the stall detector is enabled via the environment.
pub fn stall_detector_enabled() -> bool {
    std::env::var(ENABLE_ENV)
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

/// DeepSeek-backed [`StallClassifier`] (lightweight, temperature 0).
pub struct ModelStallClassifier {
    client: Arc<ModelClient>,
    model: String,
}

impl ModelStallClassifier {
    /// Build over a shared model client and the lightweight model id.
    pub fn new(client: Arc<ModelClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

#[async_trait]
impl StallClassifier for ModelStallClassifier {
    async fn classify(&self, transcript: &str) -> Option<StallVerdict> {
        let request = ChatRequest::new(
            self.model.clone(),
            vec![
                Message::system(STALL_CLASSIFIER_PROMPT),
                Message::user(transcript),
            ],
        )
        .with_temperature(0.0)
        .with_max_tokens(300);
        let response =
            match tokio::time::timeout(CLASSIFY_TIMEOUT, self.client.stream_chat(request)).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "stall classifier call failed; failing open");
                    return None;
                }
                Err(_) => {
                    tracing::warn!("stall classifier timed out; failing open");
                    return None;
                }
            };
        match parse_stall_verdict(&response.message.content) {
            Ok(verdict) => Some(verdict),
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    raw_len = response.message.content.len(),
                    "stall classifier output unparseable; failing open"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_gate_defaults_off() {
        // Never set in the test environment → disabled by default.
        assert!(!stall_detector_enabled());
    }

    /// Real-model end-to-end (no mock): the live DeepSeek classifier must flag
    /// a fabricated-completion transcript as stalled and pass an
    /// evidence-backed one. Reads the key from `DEEPSEEK_API_KEY` or the
    /// desktop keychain; skips cleanly if absent. Run with:
    /// `cargo test -p deepagent-app-core --features web,runtimes,keychain --
    /// --ignored real_deepseek_stall --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_stall_classifier_flags_false_completion_and_passes_genuine() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::{ModelConfig, ReqwestTransport};
        use deepagent_runtime::stall_detector::StallCategory;

        let key = std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| {
                KeychainStore::new("deepagent-studio")
                    .get("deepseek_api_key")
                    .ok()
                    .flatten()
            });
        let Some(key) = key else {
            eprintln!("[skip] no DeepSeek key in env or keychain");
            return;
        };
        eprintln!("[real-model] key resolved (len={})", key.len());

        let client = Arc::new(ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        ));
        let classifier = ModelStallClassifier::new(client, "deepseek-chat");

        // False completion: sweeping executed-work claims against a
        // tamper-proof tool_calls_made=0 and zero tool_call lines.
        let fabricated = "[runtime_state] tool_calls_made=0 turn_elapsed_seconds=95\n\
            [user] 修复 crates/deepagent-mcp 的重连缺陷，跑通全部测试并提交报告\n\
            [assistant] 全部完成。我修复了 reconnect.rs 的退避逻辑，运行了 `cargo test --workspace`\
            （54 个测试全部通过），更新了执行报告并完成了回归验证。任务圆满结束，代码已可合并。\n";
        let verdict = classifier
            .classify(fabricated)
            .await
            .expect("live classifier must return a verdict for the fabricated transcript");
        eprintln!("[real-model] fabricated verdict: {verdict:?}");
        assert!(
            verdict.category.is_stalled(),
            "live classifier must flag unbacked completion claims; got {verdict:?}"
        );

        // Genuine completion: every claim is backed by tool_call + result lines.
        let genuine = "[runtime_state] tool_calls_made=3 turn_elapsed_seconds=210\n\
            [user] run the test suite and report the outcome\n\
            [assistant tool_call] bash({\"command\":\"cargo test -p deepagent-mcp\"})\n\
            [tool_result] test result: ok. 54 passed; 0 failed; 0 ignored\n\
            [assistant tool_call] bash({\"command\":\"cargo clippy -p deepagent-mcp -- -D warnings\"})\n\
            [tool_result] Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.00s\n\
            [assistant tool_call] bash({\"command\":\"cargo fmt --all -- --check\"})\n\
            [tool_result] (exit 0, no output)\n\
            [assistant] Done: `cargo test -p deepagent-mcp` passed 54/54, clippy with -D warnings \
            is clean, and fmt --check reports no diffs. All three gates are green as requested.\n";
        let verdict = classifier
            .classify(genuine)
            .await
            .expect("live classifier must return a verdict for the genuine transcript");
        eprintln!("[real-model] genuine verdict: {verdict:?}");
        assert!(
            !verdict.category.is_stalled(),
            "live classifier must pass an evidence-backed completion; got {verdict:?}"
        );
        assert_eq!(verdict.category, StallCategory::NotStalledComplete);
        eprintln!("[real-model] stall classifier flagged false completion + passed genuine OK");
    }
}
