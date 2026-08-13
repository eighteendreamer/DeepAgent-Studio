//! LLM adversarial verification panel (M3 §2.2).
//!
//! Orthogonal layering (防四不married):
//! - **Framework = Claude Code**: verification runs as a read-only pass after
//!   the objective fact-gate ([`crate::verification_dispatcher`] /
//!   `CompletionGate`), never in the same decision branch — the fact-gate proves
//!   "files changed + build passes"; this panel proves "the change actually met
//!   the goal".
//! - **Method = Grok**: an adversarial *skeptic panel* — spawn `N` independent
//!   read-only skeptics (default 3, clamp 1..=5, `goal_classifier.rs`
//!   `GOAL_VERIFIER_SKEPTIC_COUNT`/`_MIN`/`_MAX`), each biased to refute, and
//!   aggregate by **majority-refute** (`aggregate_skeptic_verdicts`).
//! - **Output contract = Claude Code**: each skeptic ends with a literal
//!   `VERDICT: PASS|FAIL|PARTIAL` line (`verificationAgent.ts`), parsed here.
//!
//! Escape hatch (铁律 "审查结果只作反馈，不得变成误杀 run 的门卫"): the panel is
//! **advisory** — [`PanelOutcome`] is fed back to the agent as an observation,
//! never used to hard-fail a run. A parse failure or spawn error degrades to a
//! synthetic refute (Grok bias-to-fail lives at the per-skeptic level), but the
//! caller decides whether to nudge; it must not kill a correct run.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;

/// Default skeptic count. Grok `GOAL_VERIFIER_SKEPTIC_COUNT` = 3: a genuine
/// majority (⌈3/2⌉ = 2 not-refuted to pass) so one outlier can't decide.
pub const DEFAULT_SKEPTIC_COUNT: u32 = 3;
/// Lower/upper clamp for the skeptic count (Grok `_MIN`/`_MAX`).
pub const SKEPTIC_COUNT_MIN: u32 = 1;
pub const SKEPTIC_COUNT_MAX: u32 = 5;

/// Resolve the effective skeptic count from an optional override, clamped to
/// `[MIN, MAX]` (mirrors Grok `resolve_goal_verifier_count`). `None` → default.
pub fn resolve_skeptic_count(override_count: Option<u32>) -> u32 {
    override_count
        .unwrap_or(DEFAULT_SKEPTIC_COUNT)
        .clamp(SKEPTIC_COUNT_MIN, SKEPTIC_COUNT_MAX)
}

/// One skeptic's parsed verdict, aligned with Claude Code's PASS/FAIL/PARTIAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The skeptic could not break the change: goal met.
    Pass,
    /// The skeptic found a real defect / unmet criterion: refuted.
    Fail,
    /// Environmental limitation only (no tooling / can't run) — NOT a defect.
    Partial,
}

impl Verdict {
    /// Whether this verdict counts as a *refute* for aggregation. `Fail`
    /// refutes; `Pass` and `Partial` do not — `Partial` is an environmental
    /// limitation (CC semantics), and treating it as a refute would let a
    /// missing test runner kill a correct run (误杀更糟 baseline).
    pub fn is_refute(self) -> bool {
        matches!(self, Verdict::Fail)
    }
}

/// A single skeptic's result.
#[derive(Debug, Clone)]
pub struct SkepticVerdict {
    /// 0-based skeptic index (for logs / aggregation parity).
    pub index: u32,
    /// Parsed verdict.
    pub verdict: Verdict,
    /// The skeptic's short reason / final message (bounded by the caller).
    pub reason: String,
}

/// Parse the Claude-Code `VERDICT: PASS|FAIL|PARTIAL` contract from a skeptic's
/// final message. Scans from the end (the contract requires the verdict on the
/// last meaningful line) and is tolerant of markdown bold / trailing
/// punctuation. Returns `None` when no verdict line is present — the caller
/// then degrades to a synthetic refute (Grok bias-to-fail).
pub fn parse_verdict(text: &str) -> Option<Verdict> {
    for line in text.lines().rev() {
        let cleaned: String = line
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == ':')
            .collect();
        let upper = cleaned.to_ascii_uppercase();
        let Some(idx) = upper.find("VERDICT") else {
            continue;
        };
        let tail = upper[idx + "VERDICT".len()..].trim_start_matches([':', ' ']);
        if tail.starts_with("PASS") {
            return Some(Verdict::Pass);
        }
        if tail.starts_with("PARTIAL") {
            return Some(Verdict::Partial);
        }
        if tail.starts_with("FAIL") {
            return Some(Verdict::Fail);
        }
    }
    None
}

/// The aggregated panel outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelOutcome {
    /// The panel did not reach a refuting majority: the goal is accepted.
    Passed {
        /// Number of skeptics that refuted.
        refuted: u32,
        /// Total skeptics.
        total: u32,
    },
    /// A majority (or the sole judge) refuted: feed the gaps back to the agent.
    Refuted {
        /// Number of skeptics that refuted.
        refuted: u32,
        /// Total skeptics.
        total: u32,
        /// One-line-per-refuter gaps, for the feedback observation.
        gaps: Vec<String>,
    },
}

impl PanelOutcome {
    /// Whether the panel accepted the goal.
    pub fn passed(&self) -> bool {
        matches!(self, PanelOutcome::Passed { .. })
    }
}

/// Aggregate skeptic verdicts by **majority-refute** (Grok
/// `aggregate_skeptic_verdicts`, simplified). The goal PASSES only when a
/// strict majority did NOT refute: `not_refuted >= total/2 + 1`. A tie or a
/// refuting majority ⇒ `Refuted` (bias-to-fail: uncertain ⇒ refute).
///
/// No upstream counterpart for Grok's skeptic-0 resume "reject-gatekeeper"
/// (checked: grok goal_classifier.rs Variant-C) — this system has no persistent
/// resumed skeptic, so the whole panel votes with a plain strict majority
/// instead of excluding a cold gatekeeper. Documented deviation.
pub fn aggregate_verdicts(verdicts: &[SkepticVerdict]) -> PanelOutcome {
    let total = verdicts.len() as u32;
    if total == 0 {
        // Empty panel: nothing proved it wrong → accept (the caller only runs
        // the panel when it wants verification; a zero-skeptic config is a
        // no-op, not a refute).
        return PanelOutcome::Passed {
            refuted: 0,
            total: 0,
        };
    }
    let refuted: u32 = verdicts.iter().filter(|v| v.verdict.is_refute()).count() as u32;
    let not_refuted = total - refuted;
    let needed = total / 2 + 1;
    if not_refuted >= needed {
        PanelOutcome::Passed { refuted, total }
    } else {
        let gaps = verdicts
            .iter()
            .filter(|v| v.verdict.is_refute())
            .map(|v| format!("skeptic {}: {}", v.index, v.reason.trim()))
            .collect();
        PanelOutcome::Refuted {
            refuted,
            total,
            gaps,
        }
    }
}

/// Spawns one skeptic and returns its final message text. Production uses a
/// model-backed implementation; tests inject deterministic responses (mirrors
/// Grok's `GoalClassifierSpawner` trait for testable aggregation).
#[async_trait]
pub trait SkepticSpawner: Send + Sync {
    /// Run skeptic `index` against `prompt`, returning its final message.
    async fn spawn(&self, index: u32, prompt: &str) -> Result<String>;
}

/// The adversarial system prompt handed to every skeptic. Method = Grok
/// (`goal_verifier_prompt.md`: refute-by-default, read-only, audit not author);
/// output contract = Claude Code (`verificationAgent.ts`: end with a literal
/// `VERDICT:` line). Wording is DeepSeek-native, not copied verbatim.
pub const SKEPTIC_SYSTEM_PROMPT: &str = "You are an adversarial verification specialist. Your job \
is NOT to confirm the work — it is to try to break it and REFUTE that the objective was met. \
Default to FAIL if you are uncertain a required criterion holds: a false PASS ends the task \
wrongly and is far worse than one more iteration.\n\n\
READ-ONLY: audit the evidence the implementer already produced; do not build your own or modify \
anything. Judge whether tests are honest (drive the real shipped code) rather than hardcoded, \
mocked-out, or skipped. Do NOT invent requirements the objective did not ask for (extra edge \
cases, extra robustness) — that is the most common false refute.\n\n\
End your reply with EXACTLY one line, no markdown, no other text on it:\n\
VERDICT: PASS   (the objective is met)\n\
VERDICT: FAIL   (a real defect or unmet criterion; state it above)\n\
VERDICT: PARTIAL (only an environmental limitation blocked verification)";

/// Run the skeptic panel: spawn `count` skeptics concurrently, parse each
/// verdict (parse/spawn failure ⇒ synthetic refute), aggregate by
/// majority-refute. Advisory — the caller decides how to use the outcome and
/// must not hard-fail a run on it.
pub async fn run_skeptic_panel(
    spawner: Arc<dyn SkepticSpawner>,
    count: u32,
    prompt: &str,
) -> PanelOutcome {
    let count = resolve_skeptic_count(Some(count));
    let mut handles = Vec::with_capacity(count as usize);
    for index in 0..count {
        let spawner = spawner.clone();
        let prompt = prompt.to_string();
        handles.push(tokio::spawn(async move {
            match spawner.spawn(index, &prompt).await {
                Ok(text) => {
                    let verdict = parse_verdict(&text).unwrap_or(Verdict::Fail);
                    let reason = skeptic_reason(&text);
                    SkepticVerdict {
                        index,
                        verdict,
                        reason,
                    }
                }
                // Bias-to-fail at the per-skeptic level (Grok): a spawn error
                // degrades to a refute rather than silently passing.
                Err(error) => SkepticVerdict {
                    index,
                    verdict: Verdict::Fail,
                    reason: format!("skeptic spawn failed: {error}"),
                },
            }
        }));
    }
    let mut verdicts = Vec::with_capacity(count as usize);
    for handle in handles {
        match handle.await {
            Ok(verdict) => verdicts.push(verdict),
            Err(join_error) => verdicts.push(SkepticVerdict {
                index: verdicts.len() as u32,
                verdict: Verdict::Fail,
                reason: format!("skeptic task join failed: {join_error}"),
            }),
        }
    }
    verdicts.sort_by_key(|v| v.index);
    aggregate_verdicts(&verdicts)
}

/// Extract a bounded, single-line reason from a skeptic's message: the last
/// non-empty line that is not the VERDICT line, truncated.
fn skeptic_reason(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.to_ascii_uppercase().contains("VERDICT"))
        .unwrap_or("");
    line.chars().take(200).collect()
}

/// Production skeptic spawner: runs one read-only skeptic turn against the live
/// model (DeepSeek). Each skeptic is a single stateless chat completion with
/// the adversarial [`SKEPTIC_SYSTEM_PROMPT`]; the panel spawns several
/// concurrently. Low temperature keeps verdicts stable.
pub struct ModelSkepticSpawner {
    client: Arc<deepagent_models::ModelClient>,
    model: String,
    system_prompt: String,
}

impl ModelSkepticSpawner {
    /// Build a spawner over a model client + model id, using the default
    /// full-transcript [`SKEPTIC_SYSTEM_PROMPT`].
    pub fn new(client: Arc<deepagent_models::ModelClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
            system_prompt: SKEPTIC_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Override the skeptic system prompt (builder). The adversarial goal
    /// verifier uses [`GOAL_COVERAGE_SKEPTIC_PROMPT`], calibrated for its
    /// thinner evidence model (goal + changed-file list + final answer, no
    /// file contents).
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }
}

#[async_trait]
impl SkepticSpawner for ModelSkepticSpawner {
    async fn spawn(&self, _index: u32, prompt: &str) -> Result<String> {
        let request = deepagent_models::chat::ResponseRequest::new(
            self.model.clone(),
            vec![
                deepagent_core::message::Message::system(&self.system_prompt),
                deepagent_core::message::Message::user(prompt),
            ],
        )
        .with_temperature(0.2)
        .with_max_output_tokens(1024);
        let response = self.client.stream_response(request).await?;
        Ok(response.output_text_projection())
    }
}

/// Adversarial system prompt for the **goal-coverage** verifier path
/// (`ModelAdversarialVerifier`). Unlike [`SKEPTIC_SYSTEM_PROMPT`], this judge
/// is told explicitly that it CANNOT see file contents or run anything — only
/// the goal, the list of changed files, and the agent's final answer. It must
/// therefore refute ONLY on a concrete, positively-detected gap (a required
/// deliverable plainly absent from the changed-file list, or a final answer
/// that contradicts the goal), and PASS whenever the changes + answer are
/// consistent with the goal. Biasing to refute merely because contents are
/// invisible is the false-refute failure mode this system must avoid
/// (宁可漏过不可误杀 — CompletionGate 连环误杀 教训).
pub const GOAL_COVERAGE_SKEPTIC_PROMPT: &str = "You are an adversarial goal-coverage verifier for \
an AI coding agent. You are given three things and NOTHING else: the GOAL (the user's objective), \
the list of FILES the agent changed this run, and the agent's FINAL ANSWER. You cannot open files, \
read their contents, or run commands.\n\n\
Decide whether the goal is plausibly met by the changes + answer. Because you cannot inspect \
contents, judge COVERAGE and CONSISTENCY, not implementation detail:\n\
- REFUTE (FAIL) only when there is a CONCRETE, positively-detected gap: the goal explicitly \
requires a deliverable that is plainly missing from the changed-file list (e.g. the goal demands a \
test but no test file was changed, or demands a new module that does not appear), OR the final \
answer contradicts the goal or is internally inconsistent.\n\
- PASS otherwise. If the changed files and the final answer are consistent with the goal, PASS \
even though you cannot see the contents -- absence of visible contents is EXPECTED here and is \
NOT grounds to refute. Do NOT invent extra requirements the goal did not state.\n\n\
End your reply with EXACTLY one line, no markdown, no other text on it:\n\
VERDICT: PASS   (the goal is plausibly covered)\n\
VERDICT: FAIL   (a concrete required deliverable is missing, or the answer contradicts the goal; \
state it above)\n\
VERDICT: PARTIAL (you genuinely cannot judge coverage from the goal + file list + answer alone)";

/// The environment flag that enables the advisory adversarial verifier.
pub const ADVERSARIAL_VERIFY_ENV: &str = "DEEPAGENT_ADVERSARIAL_VERIFY";

/// Whether the adversarial goal verifier is enabled via the environment.
pub fn adversarial_verify_enabled() -> bool {
    std::env::var(ADVERSARIAL_VERIFY_ENV)
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

/// Adversarial goal verifier: adapts the skeptic panel to the runtime's
/// [`deepagent_runtime::AdversarialVerifier`] seam (§2.2). Captures the run's
/// goal at construction so `loop_engine` need only pass the final answer and
/// the changed-file evidence. Builds the panel prompt, runs `count` skeptics,
/// and maps the majority-refute [`PanelOutcome`] to an
/// [`deepagent_runtime::AdversarialVerdict`]. Fail-open by the panel's own
/// per-skeptic bias — a spawn error degrades to a refute, but the runtime caps
/// re-entries so a run is never trapped.
pub struct ModelAdversarialVerifier {
    spawner: Arc<dyn SkepticSpawner>,
    goal: String,
    count: u32,
}

impl ModelAdversarialVerifier {
    /// Build over a skeptic spawner, the run's goal text, and a skeptic count
    /// (clamped to Grok's `[MIN, MAX]`).
    pub fn new(spawner: Arc<dyn SkepticSpawner>, goal: impl Into<String>, count: u32) -> Self {
        Self {
            spawner,
            goal: goal.into(),
            count: resolve_skeptic_count(Some(count)),
        }
    }

    /// Render the per-skeptic prompt: goal + final answer + changed-file
    /// evidence. Bounded so a huge answer cannot blow the skeptic context.
    fn build_prompt(&self, final_answer: &str, changed_files: &[String]) -> String {
        const MAX_ANSWER_CHARS: usize = 4_000;
        let answer: String = if final_answer.chars().count() > MAX_ANSWER_CHARS {
            let head: String = final_answer.chars().take(MAX_ANSWER_CHARS).collect();
            format!("{head}\u{2026} [truncated]")
        } else {
            final_answer.to_string()
        };
        let files = if changed_files.is_empty() {
            "(no file mutations recorded)".to_string()
        } else {
            changed_files
                .iter()
                .take(50)
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "GOAL (the user's original objective):\n{}\n\n\
             FILES THE AGENT CHANGED THIS RUN:\n{}\n\n\
             THE AGENT'S FINAL ANSWER (its completion claim):\n{}\n\n\
             Judge whether the goal was actually met by the changes + answer above. Refute if a \
             required criterion is unproven or a completion claim lacks backing.",
            self.goal.trim(),
            files,
            answer.trim(),
        )
    }
}

#[async_trait]
impl deepagent_runtime::AdversarialVerifier for ModelAdversarialVerifier {
    async fn verify(
        &self,
        final_answer: &str,
        changed_files: &[String],
    ) -> deepagent_runtime::AdversarialVerdict {
        let prompt = self.build_prompt(final_answer, changed_files);
        match run_skeptic_panel(self.spawner.clone(), self.count, &prompt).await {
            PanelOutcome::Passed { refuted, total } => {
                tracing::info!(refuted, total, "adversarial panel accepted the completion");
                deepagent_runtime::AdversarialVerdict::Accepted
            }
            PanelOutcome::Refuted {
                refuted,
                total,
                gaps,
            } => {
                tracing::warn!(refuted, total, "adversarial panel refuted the completion");
                deepagent_runtime::AdversarialVerdict::Refuted { gaps }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn resolve_count_clamps_to_grok_bounds() {
        assert_eq!(resolve_skeptic_count(None), 3);
        assert_eq!(resolve_skeptic_count(Some(0)), 1);
        assert_eq!(resolve_skeptic_count(Some(99)), 5);
        assert_eq!(resolve_skeptic_count(Some(4)), 4);
    }

    #[test]
    fn parses_claude_code_verdict_contract() {
        assert_eq!(parse_verdict("stuff\nVERDICT: PASS"), Some(Verdict::Pass));
        assert_eq!(parse_verdict("VERDICT: FAIL\n"), Some(Verdict::Fail));
        assert_eq!(
            parse_verdict("reasons\n**VERDICT: PARTIAL**"),
            Some(Verdict::Partial)
        );
        // Last verdict line wins; tolerant of trailing punctuation.
        assert_eq!(
            parse_verdict("VERDICT: FAIL\nrethink\nVERDICT: PASS."),
            Some(Verdict::Pass)
        );
        assert_eq!(parse_verdict("no verdict here"), None);
    }

    #[test]
    fn partial_is_not_a_refute() {
        assert!(!Verdict::Pass.is_refute());
        assert!(!Verdict::Partial.is_refute());
        assert!(Verdict::Fail.is_refute());
    }

    fn v(index: u32, verdict: Verdict) -> SkepticVerdict {
        SkepticVerdict {
            index,
            verdict,
            reason: format!("r{index}"),
        }
    }

    #[test]
    fn majority_refute_aggregation() {
        // 3 skeptics, 2 pass 1 fail → passes (2 >= 2 needed).
        let out = aggregate_verdicts(&[
            v(0, Verdict::Pass),
            v(1, Verdict::Pass),
            v(2, Verdict::Fail),
        ]);
        assert!(out.passed());

        // 3 skeptics, 2 fail 1 pass → refuted (1 < 2 needed).
        let out = aggregate_verdicts(&[
            v(0, Verdict::Fail),
            v(1, Verdict::Fail),
            v(2, Verdict::Pass),
        ]);
        assert!(!out.passed());
        if let PanelOutcome::Refuted { refuted, gaps, .. } = out {
            assert_eq!(refuted, 2);
            assert_eq!(gaps.len(), 2);
        } else {
            panic!("expected Refuted");
        }

        // Sole judge refutes → refuted.
        assert!(!aggregate_verdicts(&[v(0, Verdict::Fail)]).passed());
        // Sole judge passes → passes.
        assert!(aggregate_verdicts(&[v(0, Verdict::Pass)]).passed());
        // Even panel tie (1-1) → refuted (needed = 2, not_refuted = 1).
        assert!(!aggregate_verdicts(&[v(0, Verdict::Pass), v(1, Verdict::Fail)]).passed());
    }

    struct ScriptedSpawner {
        replies: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SkepticSpawner for ScriptedSpawner {
        async fn spawn(&self, index: u32, _prompt: &str) -> Result<String> {
            let reply = self
                .replies
                .lock()
                .unwrap()
                .get(index as usize)
                .cloned()
                .unwrap_or_default();
            Ok(reply)
        }
    }

    #[tokio::test]
    async fn panel_runs_and_aggregates_injected_verdicts() {
        let spawner = Arc::new(ScriptedSpawner {
            replies: Mutex::new(vec![
                "looks good\nVERDICT: PASS".to_string(),
                "found nothing\nVERDICT: PASS".to_string(),
                "missing test for criterion 2\nVERDICT: FAIL".to_string(),
            ]),
        });
        let outcome = run_skeptic_panel(spawner, 3, "objective + evidence").await;
        assert!(outcome.passed(), "2 PASS / 1 FAIL is a passing majority");
    }

    #[tokio::test]
    async fn unparseable_skeptic_degrades_to_refute() {
        let spawner = Arc::new(ScriptedSpawner {
            replies: Mutex::new(vec![
                "no verdict at all".to_string(),
                "also nothing".to_string(),
                "VERDICT: PASS".to_string(),
            ]),
        });
        // Two unparseable → synthetic FAIL each; only one real PASS → refuted.
        let outcome = run_skeptic_panel(spawner, 3, "objective").await;
        assert!(!outcome.passed());
    }

    /// Real-model end-to-end (no mock): a live DeepSeek skeptic reviews a
    /// trivially-correct claim and the panel parses a decision. Reads the key
    /// from `DEEPSEEK_API_KEY` or the desktop keychain; skips cleanly if absent.
    /// Run with: `cargo test -p deepagent-app-core --features web,runtimes,keychain
    /// -- --ignored real_deepseek_skeptic_panel --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_skeptic_panel_reaches_a_verdict() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};

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
        let spawner: Arc<dyn SkepticSpawner> =
            Arc::new(ModelSkepticSpawner::new(client, "deepseek-chat"));

        let prompt = "OBJECTIVE: Add a function `add(a, b)` returning their sum.\n\n\
            CHANGED CODE:\nfn add(a: i32, b: i32) -> i32 { a + b }\n\n\
            EVIDENCE: a unit test `assert_eq!(add(2, 3), 5)` passes.\n\n\
            Decide whether the objective is met.";
        // Single real skeptic (N=1): the sole judge decides.
        let outcome = run_skeptic_panel(spawner, 1, prompt).await;
        eprintln!("[real-model] skeptic panel outcome: {outcome:?}");
        // The panel reached a real decision from a live verdict (not a synthetic
        // spawn-failure refute): a correct trivial change should PASS.
        assert!(
            outcome.passed(),
            "a live skeptic should PASS a trivially-correct change; got {outcome:?}"
        );
    }

    /// Real-model end-to-end (no mock): the full [`ModelAdversarialVerifier`]
    /// seam that `loop_engine` consults. A false-completion claim (goal asks
    /// for tests, answer claims success, but no test file was changed) must be
    /// REFUTED; a genuine goal+evidence claim must be ACCEPTED. Reads the key
    /// from `DEEPSEEK_API_KEY` or the desktop keychain; skips if absent. Run:
    /// `cargo test -p deepagent-app-core --features web,runtimes,keychain --
    /// --ignored real_deepseek_adversarial_verifier --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_adversarial_verifier_refutes_false_and_accepts_genuine() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
        use deepagent_runtime::{AdversarialVerdict, AdversarialVerifier};

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
        let spawner: Arc<dyn SkepticSpawner> = Arc::new(
            ModelSkepticSpawner::new(client, "deepseek-chat")
                .with_system_prompt(GOAL_COVERAGE_SKEPTIC_PROMPT),
        );

        // False completion: goal demands a passing test, answer claims success,
        // but the only changed file is the impl — no test file, no run evidence.
        let false_verifier = ModelAdversarialVerifier::new(
            spawner.clone(),
            "Add an `is_even(n)` helper AND a unit test proving it, and run the test suite green.",
            3,
        );
        let false_verdict = false_verifier
            .verify(
                "Done — implemented `is_even` and all tests pass. Everything is green.",
                &["src/util.rs".to_string()],
            )
            .await;
        eprintln!("[real-model] false-completion verdict: {false_verdict:?}");
        assert!(
            matches!(false_verdict, AdversarialVerdict::Refuted { .. }),
            "panel must refute a test-claim with no test file changed; got {false_verdict:?}"
        );

        // Genuine: goal is a simple impl, answer + changed file are consistent.
        let genuine_verifier = ModelAdversarialVerifier::new(
            spawner,
            "Add a function `add(a, b)` that returns their sum.",
            3,
        );
        let genuine_verdict = genuine_verifier
            .verify(
                "Added `fn add(a: i32, b: i32) -> i32 { a + b }` to src/math.rs as requested.",
                &["src/math.rs".to_string()],
            )
            .await;
        eprintln!("[real-model] genuine verdict: {genuine_verdict:?}");
        assert_eq!(
            genuine_verdict,
            AdversarialVerdict::Accepted,
            "panel must accept a consistent goal+evidence completion"
        );
        eprintln!("[real-model] adversarial verifier refuted false + accepted genuine OK");
    }
}
