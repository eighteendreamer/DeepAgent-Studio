//! Advisory adversarial goal verification hook (§2.2, main source Grok
//! `goal_classifier.rs` skeptic panel + Claude Code `verificationAgent.ts`
//! PASS/FAIL contract).
//!
//! # Orthogonal layering
//!
//! This is the **LLM verification (辅)** layer that runs *after* the fact-gate
//! (`VerificationPlan` proves "files changed + build passes"). Where the
//! fact-gate proves the change is mechanically sound, this panel asks the
//! deeper question — "did the change actually meet the goal?" — via an
//! adversarial skeptic panel that votes by majority-refute.
//!
//! The panel implementation and its majority-refute aggregation live in a
//! higher crate (`deepagent-app-core::verification_panel`, which owns the
//! model client and the goal text). This trait is the runtime-side seam so
//! `loop_engine` can consult it without depending on the model layer — the
//! same injection pattern as `ReactiveContextCompactor` /
//! `StallClassifier`.
//!
//! # Safety posture (自创机制默认从宽 — CompletionGate 连环误杀 教训)
//!
//! - **Advisory, never a hard-fail.** A refuting verdict only ever feeds the
//!   gaps back as one observation so the agent can address them; it can never
//!   fail the run. After the bounded retry budget the completion is accepted
//!   regardless of the verdict.
//! - **Fail-open.** Any panel error yields [`AdversarialVerdict::Accepted`].
//! - **Fact-gated.** Only runs when the run actually mutated files (same gate
//!   as the fact-based `verify_after_completion`).
//! - **Opt-in.** Wired only when explicitly enabled by the embedding layer.

use async_trait::async_trait;

/// The panel's advisory verdict on whether a completed run met its goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdversarialVerdict {
    /// The panel did not reach a refuting majority — accept the completion.
    Accepted,
    /// A majority of skeptics refuted; `gaps` is one short line per refuter,
    /// fed back to the agent as advisory feedback.
    Refuted {
        /// One line per refuting skeptic (already bounded/trimmed).
        gaps: Vec<String>,
    },
}

/// Consulted once at completion (advisory). Implementations MUST fail open:
/// return [`AdversarialVerdict::Accepted`] on any internal error so a flaky
/// panel never blocks a correct run.
#[async_trait]
pub trait AdversarialVerifier: Send + Sync {
    /// Judge whether the run's `final_answer` (with `changed_files` as the
    /// mutation evidence) met its goal. The goal itself is captured by the
    /// implementation at construction, so the runtime need not thread it.
    async fn verify(&self, final_answer: &str, changed_files: &[String]) -> AdversarialVerdict;
}
