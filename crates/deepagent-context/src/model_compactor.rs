//! Model-driven context compaction (gap-closure spec, Phase 2B).
//!
//! The built-in [`HeuristicSummarizer`](crate::compaction::HeuristicSummarizer)
//! is deterministic and offline but extractive: it keyword-matches lines rather
//! than understanding them. [`ModelCompactor`] asks the model to read the older
//! turns and return a structured JSON summary (goal / completed / pending /
//! decisions / failures), which preserves intent far better for long sessions.
//!
//! It is **async** (a model call), so it does not implement the synchronous
//! [`Summarizer`](crate::compaction::Summarizer) trait directly. Instead it
//! exposes [`ModelCompactor::summarize`] returning a [`TaskSummary`], and falls
//! back to the heuristic summarizer on any model failure (no key, network
//! error, malformed JSON) so compaction always succeeds (Requirement 4.5).

use std::sync::Arc;

use deepagent_models::{
    classify_model_error, ModelClient, ModelFailureKind, ResponseRequest,
};

use crate::compaction::{HeuristicSummarizer, Summarizer, TaskSummary};

/// Summarizes older conversation turns into a [`TaskSummary`] via a model call,
/// with a deterministic heuristic fallback.
pub struct ModelCompactor {
    client: Arc<ModelClient>,
    model: String,
    /// Sampling temperature for the summary call (low = stable).
    temperature: f32,
    /// Cap on the summary's output tokens.
    max_tokens: u32,
}

/// The compaction system prompt: instructs the model to emit ONLY JSON.
const COMPACT_SYSTEM: &str = r#"You compress an agent's older conversation turns into a compact, structured progress summary so the agent can keep working without the full transcript. Preserve intent and hard-won facts; drop chatter.

Do NOT call any tools — respond with text only. You MAY precede the JSON with a short <analysis>...</analysis> scratch block; it is stripped before parsing.

Return ONLY a single JSON object (no markdown fence, no prose outside any <analysis> block) with exactly these keys:
{
  "goal": "the overall task in one sentence",
  "completed": ["concrete things already done"],
  "pending": ["things still to do"],
  "decisions": ["notable design/technical decisions and why"],
  "failures": ["dead-ends or errors to avoid repeating"]
}
Each array holds short strings (max ~12 items). If a section is empty, use []."#;

/// Retries after a compaction request is rejected as too long (Claude Code
/// `MAX_PTL_RETRIES`). Each retry drops the oldest half of the remaining turns.
const MAX_PTL_RETRIES: usize = 3;

impl ModelCompactor {
    /// Build a compactor over a model client and model id. Defaults: temperature
    /// 0.2, max_tokens 1024.
    pub fn new(client: Arc<ModelClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
            temperature: 0.2,
            max_tokens: 1024,
        }
    }

    /// Override the sampling temperature (builder style).
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = t;
        self
    }

    /// Override the max output tokens (builder style).
    pub fn with_max_output_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    /// Summarize `older_turns` under `goal`, folding into `prior`. On any model
    /// failure this falls back to [`HeuristicSummarizer`] so the caller always
    /// gets a usable summary.
    pub async fn summarize(
        &self,
        goal: &str,
        prior: &TaskSummary,
        older_turns: &[String],
    ) -> TaskSummary {
        match self.try_model_summary(goal, prior, older_turns).await {
            Some(summary) => summary,
            None => {
                tracing::warn!("model compaction failed; falling back to heuristic summarizer");
                HeuristicSummarizer.summarize(goal, prior, older_turns)
            }
        }
    }

    /// Attempt the model-backed summary; `None` on any failure.
    ///
    /// PTL-aware retry (Claude Code `truncateHeadForPTLRetry`): if the compaction
    /// request itself is rejected as too long, drop the OLDEST half of the
    /// remaining turns and retry, up to [`MAX_PTL_RETRIES`]. Turns are already
    /// pre-rendered strings, so truncating the head cannot split a tool-call /
    /// result pair.
    async fn try_model_summary(
        &self,
        goal: &str,
        prior: &TaskSummary,
        older_turns: &[String],
    ) -> Option<TaskSummary> {
        if older_turns.is_empty() {
            return Some(prior.clone());
        }
        let mut keep_from = 0usize;
        for attempt in 0..=MAX_PTL_RETRIES {
            let slice = &older_turns[keep_from..];
            if slice.is_empty() {
                return None;
            }
            let user = build_user_prompt(goal, prior, slice);
            let request = ResponseRequest::with_instructions_and_user_input(
                self.model.clone(),
                COMPACT_SYSTEM,
                user,
            )
            .with_temperature(self.temperature)
            .with_max_output_tokens(self.max_tokens);

            match self.client.stream_response(request).await {
                Ok(response) => {
                    let content = response.output_text_projection();
                    return parse_summary_json(&content)
                        .map(|parsed| merge_into_prior(prior, parsed, goal));
                }
                Err(error) => {
                    if classify_model_error(&error) != ModelFailureKind::ContextOverflow
                        || attempt == MAX_PTL_RETRIES
                    {
                        return None;
                    }
                    let remaining = older_turns.len() - keep_from;
                    keep_from += (remaining / 2).max(1);
                    tracing::warn!(
                        attempt = attempt + 1,
                        dropped = keep_from,
                        "compaction request hit context overflow; retrying with truncated head"
                    );
                }
            }
        }
        None
    }
}

/// Build the user message: prior summary (if any) + the older turns to fold in.
fn build_user_prompt(goal: &str, prior: &TaskSummary, older_turns: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("Task goal: {goal}\n\n"));
    if prior != &TaskSummary::default() {
        s.push_str("Existing summary so far (refine and extend it):\n");
        s.push_str(&prior.to_context_block());
        s.push_str("\n\n");
    }
    s.push_str("Older conversation turns to compress:\n");
    for (i, turn) in older_turns.iter().enumerate() {
        s.push_str(&format!("--- turn {} ---\n{}\n", i + 1, turn));
    }
    s
}

/// The JSON shape the model is asked to emit.
#[derive(Debug, serde::Deserialize)]
struct SummaryJson {
    #[serde(default)]
    goal: String,
    #[serde(default)]
    completed: Vec<String>,
    #[serde(default)]
    pending: Vec<String>,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    failures: Vec<String>,
}

/// Strip a leading/embedded `<analysis>...</analysis>` scratch block (Claude
/// Code `formatCompactSummary` parity) so the model's private reasoning never
/// leaks into the parsed summary.
fn strip_analysis(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("<analysis>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</analysis>") {
            Some(end) => rest = &rest[start + end + "</analysis>".len()..],
            None => {
                // Unclosed block: drop the remainder.
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse the model output into a [`SummaryJson`], tolerating a stray markdown
/// fence or surrounding prose by extracting the first `{...}` block. Any
/// `<analysis>` scratch block is stripped first.
fn parse_summary_json(content: &str) -> Option<SummaryJson> {
    let cleaned = strip_analysis(content);
    let trimmed = cleaned.trim();
    // Fast path: whole thing is JSON.
    if let Ok(v) = serde_json::from_str::<SummaryJson>(trimmed) {
        return Some(v);
    }
    // Otherwise, slice from the first '{' to the last '}'.
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<SummaryJson>(&trimmed[start..=end]).ok()
}

/// Merge a freshly parsed summary into the prior one (extend, dedup), keeping
/// the prior goal when the model returns an empty goal.
fn merge_into_prior(prior: &TaskSummary, parsed: SummaryJson, goal: &str) -> TaskSummary {
    let mut out = prior.clone();
    if out.goal.is_empty() {
        out.goal = if parsed.goal.trim().is_empty() {
            goal.to_string()
        } else {
            parsed.goal
        };
    }
    extend_unique(&mut out.completed_steps, parsed.completed);
    extend_unique(&mut out.pending_steps, parsed.pending);
    extend_unique(&mut out.design_decisions, parsed.decisions);
    extend_unique(&mut out.known_failures, parsed.failures);
    out
}

fn extend_unique(dst: &mut Vec<String>, src: Vec<String>) {
    for item in src {
        let item = item.trim().to_string();
        if !item.is_empty() && !dst.contains(&item) {
            dst.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_models::{MockTransport, ModelConfig};

    fn client(events: Vec<String>) -> Arc<ModelClient> {
        let transport = Arc::new(MockTransport::new(events));
        Arc::new(ModelClient::new(transport, ModelConfig::deepseek("test")))
    }

    #[test]
    fn parses_plain_json() {
        let s = r#"{"goal":"build X","completed":["a"],"pending":[],"decisions":["use rust"],"failures":[]}"#;
        let v = parse_summary_json(s).unwrap();
        assert_eq!(v.goal, "build X");
        assert_eq!(v.completed, vec!["a"]);
        assert_eq!(v.decisions, vec!["use rust"]);
    }

    #[test]
    fn parses_json_inside_fence_and_prose() {
        let s = "Sure! Here is the summary:\n```json\n{\"goal\":\"g\",\"completed\":[\"x\"]}\n```\nDone.";
        let v = parse_summary_json(s).unwrap();
        assert_eq!(v.goal, "g");
        assert_eq!(v.completed, vec!["x"]);
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_summary_json("no json here").is_none());
    }

    #[test]
    fn strips_leading_analysis_block_before_parsing() {
        let s = "<analysis>\nprivate reasoning\n</analysis>\n{\"goal\":\"g\",\"completed\":[\"a\"]}";
        let v = parse_summary_json(s).unwrap();
        assert_eq!(v.goal, "g");
        assert_eq!(v.completed, vec!["a"]);
    }

    #[test]
    fn unclosed_analysis_block_drops_remainder() {
        let s = "{\"goal\":\"kept\",\"completed\":[]} trailing <analysis> never closed";
        let v = parse_summary_json(s).unwrap();
        assert_eq!(v.goal, "kept");
    }

    #[test]
    fn merge_keeps_prior_goal_and_dedups() {
        let prior = TaskSummary {
            goal: "original".into(),
            completed_steps: vec!["step1".into()],
            ..Default::default()
        };
        let parsed = SummaryJson {
            goal: "different".into(),
            completed: vec!["step1".into(), "step2".into()],
            pending: vec![],
            decisions: vec![],
            failures: vec![],
        };
        let merged = merge_into_prior(&prior, parsed, "fallback");
        // Prior goal wins (non-empty).
        assert_eq!(merged.goal, "original");
        // step1 not duplicated; step2 added.
        assert_eq!(merged.completed_steps, vec!["step1", "step2"]);
    }

    #[tokio::test]
    async fn model_summary_parses_response() {
        let events = vec![
            r#"{"type":"response.output_text.delta","delta":"{\"goal\":\"ship feature\",\"completed\":[\"wrote code\"],\"pending\":[\"tests\"],\"decisions\":[\"chose sqlite\"],\"failures\":[\"migration bug\"]}"}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
        ];
        let compactor = ModelCompactor::new(client(events), "deepseek-v4-flash");
        let summary = compactor
            .summarize(
                "ship feature",
                &TaskSummary::default(),
                &["did some work".to_string()],
            )
            .await;
        assert_eq!(summary.goal, "ship feature");
        assert!(summary
            .completed_steps
            .iter()
            .any(|s| s.contains("wrote code")));
        assert!(summary.pending_steps.iter().any(|s| s.contains("tests")));
        assert!(summary
            .design_decisions
            .iter()
            .any(|s| s.contains("sqlite")));
        assert!(summary
            .known_failures
            .iter()
            .any(|s| s.contains("migration bug")));
    }

    #[tokio::test]
    async fn falls_back_to_heuristic_on_bad_output() {
        // Model returns non-JSON prose → fall back to heuristic extraction.
        let events = vec![
            r#"{"type":"response.output_text.delta","delta":"I cannot do that."}"#.to_string(),
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
        ];
        let compactor = ModelCompactor::new(client(events), "deepseek-v4-flash");
        let summary = compactor
            .summarize(
                "build storage",
                &TaskSummary::default(),
                &["Created the database module".to_string()],
            )
            .await;
        // Heuristic picks up "Created ..." as a completed step and sets the goal.
        assert_eq!(summary.goal, "build storage");
        assert!(summary
            .completed_steps
            .iter()
            .any(|s| s.contains("Created")));
    }

    #[tokio::test]
    async fn empty_older_turns_returns_prior() {
        let compactor = ModelCompactor::new(client(vec![]), "deepseek-v4-flash");
        let prior = TaskSummary {
            goal: "g".into(),
            ..Default::default()
        };
        let summary = compactor.summarize("g", &prior, &[]).await;
        assert_eq!(summary, prior);
    }

    /// First call fails with a context-overflow (PTL) error; the retry succeeds.
    struct PtlThenOkTransport {
        calls: std::sync::atomic::AtomicUsize,
        events: Vec<String>,
    }

    #[async_trait::async_trait]
    impl deepagent_models::transport::HttpTransport for PtlThenOkTransport {
        async fn stream(
            &self,
            _request: deepagent_models::TransportRequest,
            sink: &mut dyn deepagent_models::transport::EventSink,
        ) -> deepagent_core::error::Result<()> {
            let call = self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call == 0 {
                return Err(deepagent_core::error::CoreError::provider(
                    Some(413),
                    Some("context_length_exceeded".to_string()),
                    "maximum context window exceeded".to_string(),
                ));
            }
            for event in &self.events {
                if sink.on_event(event)? {
                    break;
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn retries_compaction_on_context_overflow() {
        let transport = Arc::new(PtlThenOkTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
            events: vec![
                r#"{"type":"response.output_text.delta","delta":"{\"goal\":\"g\",\"completed\":[\"ok\"]}"}"#
                    .to_string(),
                r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
            ],
        });
        let client = Arc::new(ModelClient::new(
            transport,
            deepagent_models::ModelConfig::deepseek("test"),
        ));
        let compactor = ModelCompactor::new(client, "deepseek-flash");
        let summary = compactor
            .summarize(
                "g",
                &TaskSummary::default(),
                &["a".into(), "b".into(), "c".into(), "d".into()],
            )
            .await;
        // The PTL retry dropped the oldest half and succeeded on the second call.
        assert_eq!(summary.goal, "g");
        assert!(summary.completed_steps.iter().any(|s| s.contains("ok")));
    }
}
