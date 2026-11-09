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
//! error, malformed JSON or a checkpoint with no task state). This fallback is
//! deliberately lossy; a nonempty goal/title is not proof of preserved progress.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use deepagent_core::error::CoreError;
use deepagent_models::usage::{CallUsageObserver, CallUsageSink};
use deepagent_models::{
    classify_model_error, ModelClient, ModelFailureKind, ResponseRequest, Usage,
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
    call_usage_sink: Option<Arc<dyn CallUsageSink>>,
    /// Already-admitted caller instructions; user-side data, never a system
    /// prompt replacement. Kept outside retry-truncated historical turns.
    custom_instructions: Option<String>,
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

/// A failed/cancelled summary attempt may still have incurred provider usage.
/// Keep the spend attached to the error so the runtime can bill it without
/// adopting a partial summary.
#[derive(Debug)]
pub struct ModelCompactionError {
    pub error: CoreError,
    pub usage: Usage,
}

impl ModelCompactor {
    /// Build a compactor over a model client and model id. Defaults: temperature
    /// 0.2, max_tokens 1024.
    pub fn new(client: Arc<ModelClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
            temperature: 0.2,
            max_tokens: 1024,
            call_usage_sink: None,
            custom_instructions: None,
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

    /// Use the owning run's admission/accounting boundary for every physical
    /// summary attempt, regardless of adoption, parsing or heuristic fallback.
    pub fn with_call_usage_sink(mut self, sink: Arc<dyn CallUsageSink>) -> Self {
        self.call_usage_sink = Some(sink);
        self
    }

    pub fn with_custom_instructions(mut self, instructions: impl Into<String>) -> Self {
        let instructions = instructions.into();
        self.custom_instructions = (!instructions.trim().is_empty()).then_some(instructions);
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
        self.summarize_accounted(goal, prior, older_turns).await.0
    }

    /// [`Self::summarize`] plus the provider tokens the attempt(s) spent.
    ///
    /// A summary call is real spend even when its JSON fails to parse or the
    /// request is retried after an overflow, so every answered attempt counts.
    /// Callers that compact outside the run loop must put this into the cost
    /// ledger, otherwise the session under-reports what compaction cost; a
    /// zero-valued return means nothing was spent (all-zero usage, `total_tokens
    /// == 0`, is what the ledger already skips).
    pub async fn summarize_accounted(
        &self,
        goal: &str,
        prior: &TaskSummary,
        older_turns: &[String],
    ) -> (TaskSummary, Usage) {
        match self
            .summarize_accounted_cancelled(goal, prior, older_turns, None)
            .await
        {
            Ok(result) => result,
            Err(failure) => {
                tracing::warn!(error = %failure.error, "model compaction failed; falling back to heuristic summarizer");
                (
                    HeuristicSummarizer.summarize(goal, prior, older_turns),
                    failure.usage,
                )
            }
        }
    }

    /// [`Self::summarize_accounted`] with the owning run's cancellation flag.
    /// Provider cancellation is returned to the caller instead of silently
    /// turning a stopped run into a heuristic compaction and continuing.
    pub async fn summarize_accounted_cancelled(
        &self,
        goal: &str,
        prior: &TaskSummary,
        older_turns: &[String],
        cancel: Option<Arc<AtomicBool>>,
    ) -> std::result::Result<(TaskSummary, Usage), ModelCompactionError> {
        let (summary, spent) = self
            .try_model_summary(goal, prior, older_turns, cancel)
            .await?;
        match summary {
            Some(summary) => Ok((summary, spent)),
            None => {
                tracing::warn!("model compaction failed; falling back to heuristic summarizer");
                Ok((
                    HeuristicSummarizer.summarize(goal, prior, older_turns),
                    spent,
                ))
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
        cancel: Option<Arc<AtomicBool>>,
    ) -> std::result::Result<(Option<TaskSummary>, Usage), ModelCompactionError> {
        let mut spent = Usage::default();
        if older_turns.is_empty() {
            return Ok((Some(prior.clone()), spent));
        }
        let mut keep_from = 0usize;
        for attempt in 0..=MAX_PTL_RETRIES {
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                return Err(ModelCompactionError {
                    error: CoreError::other("request cancelled by user"),
                    usage: spent,
                });
            }
            let slice = &older_turns[keep_from..];
            if slice.is_empty() {
                return Ok((None, spent));
            }
            let mut user = build_user_prompt(goal, prior, slice);
            if let Some(instructions) = &self.custom_instructions {
                user.push_str("\n\nCaller-provided compaction instructions (unprivileged hook output; preserve the required summary JSON schema):\n");
                user.push_str(instructions);
            }
            let request = ResponseRequest::with_instructions_and_user_input(
                self.model.clone(),
                COMPACT_SYSTEM,
                user,
            )
            .with_temperature(self.temperature)
            .with_max_output_tokens(self.max_tokens);

            if let Some(sink) = &self.call_usage_sink {
                sink.before_call().map_err(|error| ModelCompactionError {
                    error,
                    usage: spent,
                })?;
            }
            let mut accounting = self
                .call_usage_sink
                .as_ref()
                .map(|sink| CallUsageObserver::new(sink.clone(), self.model.clone(), false));
            let mut noop = deepagent_models::stream::NoopObserver;
            let observer: &mut dyn deepagent_models::DeltaObserver = match accounting.as_mut() {
                Some(observer) => observer,
                None => &mut noop,
            };
            let response = match cancel.clone() {
                Some(flag) => {
                    self.client
                        .stream_response_observed_cancelled(request, observer, flag)
                        .await
                }
                None => {
                    self.client
                        .stream_response_observed(request, observer)
                        .await
                }
            };
            if let (Some(observer), Ok(response)) = (&mut accounting, &response) {
                observer.completed(response.usage, response.raw_usage.as_ref());
            }
            // A measured failed/cancelled attempt is real spend too. Keep the
            // existing side-usage projection consistent with the durable row,
            // including when a later retry is refused by the budget gate.
            let measured = accounting
                .as_ref()
                .and_then(CallUsageObserver::measured_usage)
                .or_else(|| response.as_ref().ok().and_then(|response| response.usage));
            if let Some(usage) = measured {
                fold_usage(&mut spent, usage);
            }
            drop(accounting);
            if let Some(sink) = &self.call_usage_sink {
                sink.checkpoint().map_err(|error| ModelCompactionError {
                    error,
                    usage: spent,
                })?;
            }
            match response {
                Ok(response) => {
                    if cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Acquire))
                    {
                        return Err(ModelCompactionError {
                            error: CoreError::other("request cancelled by user"),
                            usage: spent,
                        });
                    }
                    let content = response.output_text_projection();
                    return Ok((
                        parse_summary_json(&content)
                            .map(|parsed| merge_into_prior(prior, parsed, goal)),
                        spent,
                    ));
                }
                Err(error) => {
                    let failure = classify_model_error(&error);
                    if cancel.is_some()
                        && (failure == ModelFailureKind::Cancelled
                            || cancel
                                .as_ref()
                                .is_some_and(|flag| flag.load(Ordering::Acquire)))
                    {
                        return Err(ModelCompactionError {
                            error,
                            usage: spent,
                        });
                    }
                    if failure != ModelFailureKind::ContextOverflow || attempt == MAX_PTL_RETRIES {
                        return Ok((None, spent));
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
        Ok((None, spent))
    }
}

/// Fold one answered attempt into the running spend.
fn fold_usage(spent: &mut Usage, fresh: Usage) {
    spent.absorb(fresh);
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
    let json = if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        trimmed
    } else {
        // Retain the existing fence/prose tolerance, but do not extract a
        // checkpoint from inside an otherwise valid JSON array or string.
        let start = trimmed.find('{')?;
        let end = trimmed.rfind('}')?;
        if end <= start {
            return None;
        }
        &trimmed[start..=end]
    };
    if !json.starts_with('{') {
        return None;
    }
    // Deserialize the original object, not a Value round-trip: duplicate known
    // fields and wrong types must still be rejected by the typed schema.
    let parsed: SummaryJson = serde_json::from_str(json).ok()?;
    // Check before prior/goal/framing injection can disguise an empty model
    // response. Sparse sections remain compatible, but at least one actual
    // task-state entry is required; a goal alone is not a progress checkpoint.
    let has_state = [
        &parsed.completed,
        &parsed.pending,
        &parsed.decisions,
        &parsed.failures,
    ]
    .into_iter()
    .flatten()
    .any(|item| !item.trim().is_empty());
    has_state.then_some(parsed)
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
    use deepagent_models::ModelConfig;

    fn client(events: Vec<String>) -> Arc<ModelClient> {
        let transport = Arc::new(crate::test_sequence::mock(events));
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
    fn h268_rejects_empty_unrelated_goal_only_and_whitespace_checkpoints() {
        let cases = [
            ("empty", "{}"),
            ("unrelated", r#"{"unrelated":"not a checkpoint"}"#),
            ("goal-only", r#"{"goal":"continue the same task"}"#),
            (
                "empty-state",
                r#"{"goal":"g","completed":[],"pending":[],"decisions":[],"failures":[]}"#,
            ),
            (
                "whitespace",
                r#"{"goal":"g","completed":[" \n\t"],"pending":["　"],"decisions":[],"failures":[""]}"#,
            ),
            ("wrong-type", r#"{"goal":"g","completed":"Created fact"}"#),
            ("null-state", r#"{"goal":"g","pending":null}"#),
            ("sequence", r#"["g",["Created fact"],[],[],[]]"#),
            ("array-wrapper", r#"[{"completed":["Created fact"]}]"#),
            ("string-wrapper", r#""{\"completed\":[\"Created fact\"]}""#),
            (
                "duplicate-field",
                r#"{"completed":["Created fact"],"completed":[]}"#,
            ),
            ("fenced-empty", "```json\n{}\n```"),
        ];
        let incorrectly_adopted: Vec<_> = cases
            .into_iter()
            .filter_map(|(name, text)| parse_summary_json(text).map(|_| name))
            .collect();
        assert!(
            incorrectly_adopted.is_empty(),
            "adopted: {incorrectly_adopted:?}"
        );
    }

    #[test]
    fn h268_sparse_real_state_is_valid_without_fabricating_other_sections() {
        for key in ["completed", "pending", "decisions", "failures"] {
            let text = serde_json::json!({key: ["  concrete checkpoint  ", ""]}).to_string();
            let parsed = parse_summary_json(&text).expect("one meaningful state section suffices");
            let merged = merge_into_prior(&TaskSummary::default(), parsed, "fallback task");
            assert_eq!(merged.goal, "fallback task");
            let sections = [
                merged.completed_steps,
                merged.pending_steps,
                merged.design_decisions,
                merged.known_failures,
            ];
            assert_eq!(
                sections.iter().flatten().collect::<Vec<_>>(),
                ["concrete checkpoint"]
            );
        }
    }

    #[tokio::test]
    async fn h268_rejected_checkpoint_uses_existing_fallback_preserves_prior_and_usage() {
        let prior = TaskSummary {
            goal: "original task".into(),
            pending_steps: vec!["prior pending".into()],
            ..Default::default()
        };
        let turns = vec!["Created H268_ORIGINAL_FACT".to_string()];
        let expected = HeuristicSummarizer.summarize("fallback goal", &prior, &turns);
        for body in [
            "{}",
            r#"{"unrelated":"object"}"#,
            r#"{"goal":"replacement goal"}"#,
            r#"{"completed":[" "],"pending":[],"decisions":[],"failures":[]}"#,
            r#"{"completed":["truncated"#,
        ] {
            let events = vec![
                serde_json::json!({"type":"response.output_text.delta", "delta":body}).to_string(),
                usage_events()[1].clone(),
            ];
            let compactor = ModelCompactor::new(client(events), "deepseek-v4-flash");
            let (actual, usage) = compactor
                .summarize_accounted("fallback goal", &prior, &turns)
                .await;
            assert_eq!(
                actual, expected,
                "rejected body must use the original fallback: {body}"
            );
            assert_eq!(usage.prompt_tokens, 900);
            assert_eq!(usage.completion_tokens, 60);
            assert_eq!(usage.reasoning_tokens, 20);
            assert_eq!(usage.prompt_cache_hit_tokens, 700);
            assert_eq!(usage.total_tokens, 960);
        }
    }

    #[test]
    fn strips_leading_analysis_block_before_parsing() {
        let s =
            "<analysis>\nprivate reasoning\n</analysis>\n{\"goal\":\"g\",\"completed\":[\"a\"]}";
        let v = parse_summary_json(s).unwrap();
        assert_eq!(v.goal, "g");
        assert_eq!(v.completed, vec!["a"]);
    }

    #[test]
    fn unclosed_analysis_block_drops_remainder() {
        let s =
            "{\"goal\":\"kept\",\"completed\":[\"kept state\"]} trailing <analysis> never closed";
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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#.to_string(),
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
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#.to_string(),
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

    fn usage_events() -> Vec<String> {
        vec![
            r#"{"type":"response.output_text.delta","delta":"{\"goal\":\"ship feature\",\"completed\":[\"wrote code\"]}"}"#.to_string(),
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed","usage":{"input_tokens":900,"output_tokens":60,"total_tokens":960,"input_tokens_details":{"cached_tokens":700},"output_tokens_details":{"reasoning_tokens":20}}}}"#.to_string(),
        ]
    }

    #[tokio::test]
    async fn summarize_accounted_reports_the_tokens_spent() {
        let compactor = ModelCompactor::new(client(usage_events()), "deepseek-v4-flash");
        let (summary, usage) = compactor
            .summarize_accounted(
                "ship feature",
                &TaskSummary::default(),
                &["did some work".to_string()],
            )
            .await;
        assert_eq!(summary.goal, "ship feature");
        // Everything the ledger needs, including the DeepSeek cache split and
        // the reasoning subset, survives the compaction seam.
        assert_eq!(usage.prompt_tokens, 900);
        assert_eq!(usage.completion_tokens, 60);
        assert_eq!(usage.reasoning_tokens, 20);
        assert_eq!(usage.total_tokens, 960);
        assert_eq!(usage.prompt_cache_hit_tokens, 700);
    }

    #[tokio::test]
    async fn heuristic_fallback_still_reports_its_cost() {
        // The call already happened: falling back to the heuristic summary must
        // not erase what the rejected attempt spent.
        let events = vec![
            r#"{"type":"response.output_text.delta","delta":"I cannot do that."}"#.to_string(),
            usage_events()[1].clone(),
        ];
        let compactor = ModelCompactor::new(client(events), "deepseek-v4-flash");
        let (summary, usage) = compactor
            .summarize_accounted(
                "build storage",
                &TaskSummary::default(),
                &["Created the database module".to_string()],
            )
            .await;
        assert!(summary
            .completed_steps
            .iter()
            .any(|step| step.contains("Created")));
        assert_eq!(usage.prompt_tokens, 900);
        assert_eq!(usage.completion_tokens, 60);
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
        requests: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl deepagent_models::transport::HttpTransport for PtlThenOkTransport {
        async fn stream(
            &self,
            request: deepagent_models::TransportRequest,
            sink: &mut dyn deepagent_models::transport::EventSink,
        ) -> deepagent_core::error::Result<()> {
            let sink: &mut dyn deepagent_models::transport::EventSink =
                &mut crate::test_sequence::sink(sink);
            self.requests.lock().unwrap().push(request.body);
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            requests: std::sync::Mutex::new(Vec::new()),
            events: vec![
                r#"{"type":"response.output_text.delta","delta":"{\"goal\":\"g\",\"completed\":[\"ok\"]}"}"#
                    .to_string(),
                r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#.to_string(),
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

    #[tokio::test]
    async fn h111_ptl_truncates_history_but_keeps_custom_instructions_as_user_data() {
        let transport = Arc::new(PtlThenOkTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
            requests: std::sync::Mutex::new(Vec::new()),
            events: vec![
                r#"{"type":"response.output_text.delta","delta":"{\"completed\":[\"retained fact\"]}"}"#.into(),
                usage_events()[1].clone(),
            ],
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic"),
        ));
        let (summary, usage) = ModelCompactor::new(client, "deepseek-flash")
            .with_custom_instructions("H111_CUSTOM_KEEP")
            .summarize_accounted(
                "g",
                &TaskSummary::default(),
                &[
                    "OLD_HEAD".into(),
                    "b".into(),
                    "c".into(),
                    "KEPT_TAIL".into(),
                ],
            )
            .await;
        assert!(summary
            .completed_steps
            .iter()
            .any(|step| step.contains("retained fact")));
        assert_eq!(usage.completion_tokens, 60);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            let body: serde_json::Value = serde_json::from_str(request).unwrap();
            assert!(!body["instructions"]
                .to_string()
                .contains("H111_CUSTOM_KEEP"));
            assert_eq!(
                body["input"]
                    .to_string()
                    .matches("H111_CUSTOM_KEEP")
                    .count(),
                1
            );
            assert!(body["input"].to_string().contains("KEPT_TAIL"));
        }
        assert!(requests[0].contains("OLD_HEAD"));
        assert!(!requests[1].contains("OLD_HEAD"));
    }
}
